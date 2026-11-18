use crate::util;
use worker::*;

pub const PAGE_SIZE: usize = 20;
// Keep offsets exact in JavaScript and bound the work requested by a URL.
pub const MAX_PAGE: u32 = 1_000_000;

#[derive(Clone)]
pub struct PageRequest {
    pub number: u32,
    parameter: &'static str,
}

impl PageRequest {
    pub fn new(req: &Request, parameter: &'static str) -> Self {
        let number = util::query_param(req, parameter)
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|value| value.parse::<u32>().unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE))
            .unwrap_or(1);
        Self { number, parameter }
    }

    pub fn offset(&self) -> i64 {
        i64::from(self.number - 1) * PAGE_SIZE as i64
    }

    pub fn finish<T>(
        &self,
        req: &Request,
        rows: &mut Vec<T>,
        fragment: Option<&str>,
    ) -> Result<Pagination> {
        let has_next = rows.len() > PAGE_SIZE;
        rows.truncate(PAGE_SIZE);
        Ok(Pagination {
            status: format!("Page {}", self.number),
            page: self.number,
            previous: if self.number > 1 {
                Some(query_url(req, &[(self.parameter, Some(&(self.number - 1).to_string()))], fragment)?)
            } else {
                None
            },
            next: if has_next && self.number < MAX_PAGE {
                Some(query_url(req, &[(self.parameter, Some(&(self.number + 1).to_string()))], fragment)?)
            } else {
                None
            },
        })
    }
}

#[derive(Clone)]
pub struct Pagination {
    pub page: u32,
    pub status: String,
    pub previous: Option<String>,
    pub next: Option<String>,
}

/// Replace only the named query parameters, retaining filters and branch names.
/// Return a local URL so generated links cannot change the request's origin.
pub fn query_url(
    req: &Request,
    changes: &[(&str, Option<&str>)],
    fragment: Option<&str>,
) -> Result<String> {
    let mut url = req.url()?;
    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !changes.iter().any(|(name, _)| key.as_ref() == *name))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        query.extend_pairs(retained);
        for (key, value) in changes {
            if let Some(value) = value {
                query.append_pair(key, value);
            }
        }
    }
    url.set_fragment(fragment);
    let mut path = url.path().to_string();
    if let Some(query) = url.query().filter(|query| !query.is_empty()) {
        path.push('?');
        path.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        path.push('#');
        path.push_str(fragment);
    }
    Ok(path)
}
