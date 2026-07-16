use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::model::Issue;

/// Compact relative time: `now`, `5m`, `2h`, `3d`.
pub fn relative_secs(secs: u64, zero: &str) -> String {
    if secs < 60 {
        zero.to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

pub fn relative_time(ts: &str) -> String {
    let Ok(t) = DateTime::parse_from_rfc3339(ts) else {
        return "?".to_string();
    };
    let secs = (Utc::now() - t.with_timezone(&Utc)).num_seconds().max(0) as u64;
    relative_secs(secs, "now")
}

/// Aligned listing (§7.2). Displayed children render indented beneath their
/// parent when the parent is displayed too; otherwise flat with a
/// `(sub of #N)` suffix.
pub fn render_list(issues: &[Issue]) -> String {
    let displayed: HashSet<i64> = issues.iter().map(|i| i.id).collect();
    let mut children: HashMap<i64, Vec<&Issue>> = HashMap::new();
    let mut top: Vec<&Issue> = Vec::new();
    for i in issues {
        match i.parent_id {
            Some(p) if displayed.contains(&p) => children.entry(p).or_default().push(i),
            _ => top.push(i),
        }
    }
    let sort_key = |i: &&Issue| (i.status.group(), std::cmp::Reverse(i.updated_at.clone()));
    top.sort_by_key(sort_key);
    for v in children.values_mut() {
        v.sort_by_key(sort_key);
    }

    // rows: (depth, issue, "sub of" suffix)
    let mut rows: Vec<(usize, &Issue, Option<i64>)> = Vec::new();
    let mut rendered: HashSet<i64> = HashSet::new();
    let mut stack: Vec<(usize, &Issue)> = top.iter().rev().map(|i| (0, *i)).collect();
    while let Some((depth, i)) = stack.pop() {
        if !rendered.insert(i.id) {
            continue;
        }
        rows.push((depth, i, if depth == 0 { i.parent_id } else { None }));
        if let Some(kids) = children.get(&i.id) {
            for k in kids.iter().rev() {
                stack.push((depth + 1, k));
            }
        }
    }
    // Parent cycles never reach the stack from `top`; show survivors flat.
    for i in issues {
        if rendered.insert(i.id) {
            rows.push((0, i, i.parent_id));
        }
    }

    let title_of = |depth: usize, i: &Issue, sub: Option<i64>| {
        let mut t = format!("{}{}", "  ".repeat(depth), i.title);
        if let Some(p) = sub {
            t.push_str(&format!(" (sub of #{p})"));
        }
        t
    };
    let id_w = rows.iter().map(|(_, i, _)| format!("#{}", i.id).len()).max().unwrap_or(2);
    let title_w = rows
        .iter()
        .map(|(d, i, s)| title_of(*d, i, *s).chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for (depth, i, sub) in &rows {
        out.push_str(&format!(
            "{:>id_w$}  {:<11}  {:<title_w$}  {}\n",
            format!("#{}", i.id),
            i.status.as_str(),
            title_of(*depth, i, *sub),
            relative_time(&i.updated_at),
        ));
    }
    out
}

/// Grouped, ripgrep-style search output (§7.8). Returns `None` on no matches.
/// Body line numbers are 1-based over the raw body, identical to `read`.
pub fn render_grep(issues: &[&Issue], re: &Regex, context: usize) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    for issue in issues {
        let title_hit = re.is_match(&issue.title);
        let lines: Vec<&str> = issue.body.lines().collect();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| re.is_match(l))
            .map(|(n, _)| n)
            .collect();
        if !title_hit && hits.is_empty() {
            continue;
        }
        let mut s = format!("#{} {} [{}]\n", issue.id, issue.title, issue.status);
        if title_hit {
            s.push_str(&format!("  title: {}\n", issue.title));
        }
        // line index -> is a match (vs context-only)
        let mut show: BTreeMap<usize, bool> = BTreeMap::new();
        for &h in &hits {
            let lo = h.saturating_sub(context);
            let hi = (h + context).min(lines.len() - 1);
            for n in lo..=hi {
                show.entry(n).or_insert(false);
            }
            show.insert(h, true);
        }
        for (n, is_hit) in show {
            s.push_str(&format!("  {}{} {}\n", n + 1, if is_hit { ':' } else { '-' }, lines[n]));
        }
        sections.push(s);
    }
    if sections.is_empty() { None } else { Some(sections.join("\n")) }
}
