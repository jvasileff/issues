use diffy::{ConflictStyle, MergeOptions};

pub enum MergeOutcome {
    Clean(String),
    Conflict(String),
}

/// diffy's fixed conflict labels, and the ones we show instead. The draft is
/// resolved by a human reading it, so name the sides by what they mean here
/// rather than by the merge algorithm's generic roles.
const LABELS: [(&str, &str); 3] = [
    ("<<<<<<< ours", "<<<<<<< your edit"),
    ("||||||| original", "||||||| base"),
    (">>>>>>> theirs", ">>>>>>> concurrent change"),
];

/// 3-way merge of checkout-format texts.
pub fn three_way(base: &str, ours: &str, theirs: &str) -> MergeOutcome {
    let mut opts = MergeOptions::new();
    opts.set_conflict_style(ConflictStyle::Diff3);
    match opts.merge(base, ours, theirs) {
        Ok(merged) => MergeOutcome::Clean(merged),
        Err(conflicted) => MergeOutcome::Conflict(relabel(&conflicted)),
    }
}

/// Rewrite diffy's conflict labels to ours. Only whole lines that match a
/// marker exactly are touched, so body text that merely starts with angle
/// brackets is left alone. If diffy ever changes its labels this silently
/// does nothing - the markers stay valid, just generically named - and the
/// conflict test below fails to say so.
fn relabel(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let (content, eol) = match line.strip_suffix('\n') {
                Some(rest) => (rest, "\n"),
                None => (line, ""),
            };
            match LABELS.iter().find(|(from, _)| *from == content) {
                Some((_, to)) => format!("{to}{eol}"),
                None => line.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_edits_merge_cleanly() {
        let out = three_way("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n");
        match out {
            MergeOutcome::Clean(text) => assert_eq!(text, "A\nb\nC\n"),
            MergeOutcome::Conflict(text) => panic!("unexpected conflict: {text}"),
        }
    }

    /// Also guards the relabelling against a diffy change: the assertions are
    /// on our labels, not diffy's.
    fn conflict_text() -> String {
        match three_way("shared\n", "ours\n", "theirs\n") {
            MergeOutcome::Conflict(text) => text,
            MergeOutcome::Clean(text) => panic!("expected a conflict, got: {text}"),
        }
    }

    #[test]
    fn overlapping_edits_conflict_with_our_labels() {
        let text = conflict_text();
        for (from, to) in LABELS {
            assert!(text.contains(to), "missing {to:?} in:\n{text}");
            assert!(!text.contains(from), "left diffy's {from:?} in:\n{text}");
        }
    }

    #[test]
    fn conflict_keeps_both_sides_and_the_base() {
        let text = conflict_text();
        for line in ["ours", "theirs", "shared"] {
            assert!(text.contains(line), "missing {line:?} in:\n{text}");
        }
    }
}
