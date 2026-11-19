use crate::git_storage::{is_object_id, RepoHandle};
use crate::models::Repo;
use crate::i18n::DiscussionText;
use crate::native_git::{NativeRepo, SnapshotRead};
use crate::refs::{self, RefsError};
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::rc::Rc;

const MAX_READS: usize = 80;
const MAX_COMMITS: usize = 512;
const MAX_EDGES: usize = 1024;
const BASE: u8 = 1;
const HEAD: u8 = 2;
const COMMON: u8 = BASE | HEAD;
const STALE: u8 = 4;
const SEARCHED: u8 = 8;

pub(crate) enum BaseError {
    Missing,
    Unrelated,
    Ambiguous,
    Limited,
    Storage(worker::Error),
}

impl BaseError {
    pub(crate) fn message(self) -> DiscussionText {
        match self {
            Self::Missing => DiscussionText::MergeBaseMissing,
            Self::Unrelated => DiscussionText::MergeBaseUnrelated,
            Self::Ambiguous => DiscussionText::MergeBaseAmbiguous,
            Self::Limited => DiscussionText::MergeBaseLimited,
            Self::Storage(error) => {
                worker::console_log!("pull comparison: could not read Git history: {error}");
                DiscussionText::GitUnavailable
            }
        }
    }
}

/// Keep one handle for same-repository comparisons and one for each store otherwise.
pub(crate) struct Repositories {
    pub target: RepoHandle,
    source: Option<RepoHandle>,
}

impl Repositories {
    pub async fn open(env: &worker::Env, target: &Repo, source: &Repo) -> worker::Result<Self> {
        let same = target.id == source.id;
        let target = RepoHandle::open(env, target).await?;
        let source = if same { None } else { Some(RepoHandle::open(env, source).await?) };
        Ok(Self { target, source })
    }

    pub fn source(&self) -> &RepoHandle {
        self.source.as_ref().unwrap_or(&self.target)
    }
}

/// Sample exact branch tips, sharing a single advertisement for the same repository.
pub(crate) async fn branch_tips(
    repos: &Repositories,
    base_branch: &str,
    head_branch: &str,
) -> Result<(String, String), DiscussionText> {
    if repos.source.is_none() && base_branch == head_branch {
        return Err(DiscussionText::BranchesMustDiffer);
    }
    let target = load_refs(&repos.target).await?;
    let (mut base, mut head) = (None, None);
    for branch in target.branches {
        if branch.name == base_branch {
            base = Some(branch.oid);
        } else if repos.source.is_none() && branch.name == head_branch {
            head = Some(branch.oid);
        }
        if base.is_some() && (repos.source.is_some() || head.is_some()) {
            break;
        }
    }
    let base = base.ok_or(DiscussionText::BaseBranchNotFound)?;
    if let Some(source) = &repos.source {
        head = load_refs(source).await?.branches.into_iter()
            .find(|branch| branch.name == head_branch).map(|branch| branch.oid);
    }
    Ok((base, head.ok_or(DiscussionText::HeadBranchNotFound)?))
}

async fn load_refs(repo: &RepoHandle) -> Result<refs::Inventory, DiscussionText> {
    refs::load(repo).await.map_err(|error| match error {
        RefsError::Limit => DiscussionText::CompareRefsLimited,
        RefsError::Unavailable => DiscussionText::GitUnavailable,
    })
}

/// Find the unique best common ancestor. Exhaustion or incomplete history
/// must not turn a two-tip comparison into an apparent source-branch diff.
pub(crate) async fn find(repos: &Repositories, base: &str, head: &str) -> Result<String, BaseError> {
    if !is_object_id(base) || !is_object_id(head) {
        return Err(BaseError::Missing);
    }
    if let Some((target, source)) = NativeRepo::comparison_pair(&repos.target, repos.source()) {
        return match target.merge_bases(source, base, head).await.map_err(BaseError::Storage)? {
            SnapshotRead::Missing => Err(BaseError::Missing),
            SnapshotRead::Limited => Err(BaseError::Limited),
            SnapshotRead::Found(mut bases) => match bases.len() {
                0 => Err(BaseError::Unrelated),
                1 => {
                    let hash = bases.pop().ok_or(BaseError::Missing)?;
                    if !is_object_id(&hash) || hash.bytes().all(|byte| byte == b'0') {
                        return Err(BaseError::Storage(worker::Error::RustError("Native Git origin returned an invalid merge base".into())));
                    }
                    Ok(hash)
                }
                _ => Err(BaseError::Ambiguous),
            },
        };
    }
    if base == head {
        repos.target.read_commit(base).await.map_err(BaseError::Storage)?.ok_or(BaseError::Missing)?;
        if let Some(source) = &repos.source {
            source.read_commit(head).await.map_err(BaseError::Storage)?.ok_or(BaseError::Missing)?;
        }
        return Ok(head.to_owned());
    }
    let mut graph = Graph::default();
    let base = graph.intern(base.to_owned())?;
    let head = graph.intern(head.to_owned())?;
    graph.nodes[base].flags = BASE;
    graph.nodes[head].flags = HEAD;
    let mut queue = VecDeque::from([base, head]);
    while let Some(id) = queue.pop_front() {
        let side = graph.nodes[id].flags;
        if side != BASE && side != HEAD {
            continue;
        }
        let repo = if side == BASE { &repos.target } else { repos.source() };
        for edge in graph.parents(repo, id).await? {
            let parent = graph.edges[edge];
            if graph.nodes[parent].flags & (side | STALE) != 0 {
                continue;
            }
            graph.nodes[parent].flags |= side;
            if graph.nodes[parent].flags == COMMON {
                // Ancestors of a common commit cannot be better merge bases.
                // Prune any paths the other side has already walked past it.
                graph.prune_known_parents(parent);
            } else {
                queue.push_back(parent);
            }
        }
    }

    let mut remaining = graph.candidates().count();
    if remaining == 0 {
        return Err(BaseError::Unrelated);
    }
    if remaining == 1 {
        let first = graph.candidates().next().ok_or(BaseError::Missing)?;
        graph.parents(&repos.target, first).await?;
        return Ok(graph.into_hash(first));
    }
    // Parallel paths can meet again below an earlier candidate. Follow the
    // candidates' parents to eliminate those ancestors before declaring a tie.
    for id in 0..graph.nodes.len() {
        if graph.nodes[id].flags == COMMON {
            queue.extend(graph.parents(&repos.target, id).await?.map(|edge| graph.edges[edge]));
        }
    }
    while let Some(id) = queue.pop_front() {
        if graph.nodes[id].flags & SEARCHED != 0 {
            continue;
        }
        if graph.nodes[id].flags == COMMON {
            remaining -= 1;
        }
        graph.nodes[id].flags |= STALE | SEARCHED;
        if remaining == 1 {
            let best = graph.candidates().next().ok_or(BaseError::Missing)?;
            return Ok(graph.into_hash(best));
        }
        queue.extend(graph.parents(&repos.target, id).await?.map(|edge| graph.edges[edge]));
    }
    Err(BaseError::Ambiguous)
}

#[derive(Default)]
struct Graph {
    index: HashMap<Rc<String>, usize>,
    nodes: Vec<Node>,
    edges: Vec<usize>,
    reads: usize,
}

struct Node {
    hash: Rc<String>,
    parents: Option<Range<usize>>,
    flags: u8,
}

impl Graph {
    fn intern(&mut self, hash: String) -> Result<usize, BaseError> {
        if let Some(&id) = self.index.get(&hash) {
            return Ok(id);
        }
        let id = self.nodes.len();
        if id == MAX_COMMITS {
            return Err(BaseError::Limited);
        }
        let hash = Rc::new(hash);
        self.index.insert(Rc::clone(&hash), id);
        self.nodes.push(Node { hash, parents: None, flags: 0 });
        Ok(id)
    }

    async fn parents(&mut self, repo: &RepoHandle, id: usize) -> Result<Range<usize>, BaseError> {
        if let Some(parents) = self.nodes[id].parents.clone() {
            return Ok(parents);
        }
        if self.reads == MAX_READS {
            return Err(BaseError::Limited);
        }
        self.reads += 1;
        let parents = repo.commit_parents(&self.nodes[id].hash).await
            .map_err(BaseError::Storage)?.ok_or(BaseError::Missing)?;
        let start = self.edges.len();
        if parents.len() > MAX_EDGES - start {
            return Err(BaseError::Limited);
        }
        for parent in parents {
            let parent = self.intern(parent)?;
            self.edges.push(parent);
        }
        let range = start..self.edges.len();
        self.nodes[id].parents = Some(range.clone());
        Ok(range)
    }

    fn prune_known_parents(&mut self, id: usize) {
        if let Some(parents) = self.nodes[id].parents.clone() {
            for edge in parents {
                let parent = self.edges[edge];
                if self.nodes[parent].flags & STALE == 0 {
                    self.nodes[parent].flags |= STALE;
                    self.prune_known_parents(parent);
                }
            }
        }
    }

    fn candidates(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter_map(|(id, node)| (node.flags == COMMON).then_some(id))
    }

    fn into_hash(mut self, id: usize) -> String {
        drop(self.index);
        Rc::unwrap_or_clone(self.nodes.swap_remove(id).hash)
    }
}
