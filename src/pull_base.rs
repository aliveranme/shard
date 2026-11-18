use crate::artifacts::{is_object_id, RepoHandle};
use crate::i18n::DiscussionText;
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

/// Capture both exact branch tips from the same Git advertisement.
pub(crate) async fn branch_tips(
    repo: &RepoHandle,
    base_branch: &str,
    head_branch: &str,
) -> Result<(String, String), DiscussionText> {
    if base_branch == head_branch {
        return Err(DiscussionText::BranchesMustDiffer);
    }
    let inventory = refs::load(repo).await.map_err(|error| match error {
        RefsError::Limit => DiscussionText::CompareRefsLimited,
        RefsError::Unavailable => DiscussionText::GitUnavailable,
    })?;
    let (mut base, mut head) = (None, None);
    for branch in inventory.branches {
        if branch.name == base_branch {
            base = Some(branch.oid);
        } else if branch.name == head_branch {
            head = Some(branch.oid);
        }
        if base.is_some() && head.is_some() {
            break;
        }
    }
    Ok((
        base.ok_or(DiscussionText::BaseBranchNotFound)?,
        head.ok_or(DiscussionText::HeadBranchNotFound)?,
    ))
}

/// Find the unique best common ancestor. Exhaustion or incomplete history
/// must not turn a two-tip comparison into an apparent source-branch diff.
pub(crate) async fn find(repo: &RepoHandle, base: &str, head: &str) -> Result<String, BaseError> {
    if !is_object_id(base) || !is_object_id(head) {
        return Err(BaseError::Missing);
    }
    if base == head {
        repo.read_commit(head).await.map_err(BaseError::Storage)?.ok_or(BaseError::Missing)?;
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
        graph.parents(repo, first).await?;
        return Ok(graph.into_hash(first));
    }
    // Parallel paths can meet again below an earlier candidate. Follow the
    // candidates' parents to eliminate those ancestors before declaring a tie.
    for id in 0..graph.nodes.len() {
        if graph.nodes[id].flags == COMMON {
            queue.extend(graph.parents(repo, id).await?.map(|edge| graph.edges[edge]));
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
        queue.extend(graph.parents(repo, id).await?.map(|edge| graph.edges[edge]));
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
