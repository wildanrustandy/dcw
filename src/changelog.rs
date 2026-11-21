// changelog.rs — release notes compiled into the binary, so `dcw update` can say what
// changed instead of only bumping a version number.
//
// The notes are part of the binary on purpose: a machine that installed dcw needs no
// changelog file, no git checkout, and no second network request to see them.
//
// One subtlety drives update()'s design: the running binary's embedded copy predates the
// release it is installing, so it cannot describe that release. update() therefore reads
// the notes from the binary it just downloaded (by running `--version` on it), and this
// module is what that new binary answers with.

const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// The first dotted-numeric token in `s` — "dcw 0.2.9" and "0.2.9 — 2026-11-21" both
/// yield "0.2.9", and a tag's leading `v` is dropped so `v0.2.7` works too. None when
/// there is no version in it at all.
pub fn parse_version(s: &str) -> Option<String> {
    s.split_whitespace()
        // A tag's leading `v` is dropped before matching, so `v0.2.7` works too.
        .map(|t| t.trim_start_matches(['v', 'V']))
        .find(|t| t.contains('.') && t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(|t| t.trim_end_matches('.').to_string())
        .filter(|t| !t.is_empty())
}

/// (version, body) for every `## <version>` section, in file order (newest first).
fn sections() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    for line in CHANGELOG.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(s) = cur.take() {
                out.push(s);
            }
            cur = parse_version(heading).map(|v| (v, String::new()));
            continue;
        }
        if let Some((_, body)) = cur.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(s) = cur.take() {
        out.push(s);
    }
    out
}

fn trimmed(body: &str) -> String {
    body.trim().to_string()
}

/// The newest release's notes, or None when the changelog has no version section.
pub fn latest() -> Option<(String, String)> {
    sections().into_iter().next().map(|(v, b)| (v, trimmed(&b)))
}

/// Notes for every release newer than `from`, newest first — so a user jumping several
/// versions sees each one. None when the changelog does not mention `from` at all (a dev
/// build, or a version older than the file's oldest entry): the caller then reports the
/// latest release alone rather than inventing a range.
pub fn since(from: &str) -> Option<String> {
    let all = sections();
    if !all.iter().any(|(v, _)| v == from) {
        return None;
    }
    let mut body = String::new();
    for (v, b) in &all {
        if v == from {
            break;
        }
        body.push_str(&format!("dcw {v}\n\n"));
        body.push_str(&trimmed(b));
        body.push('\n');
    }
    Some(body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_out_of_version_lines_and_headings() {
        assert_eq!(parse_version("dcw 0.2.9"), Some("0.2.9".into()));
        assert_eq!(parse_version("0.2.9 — 2026-11-21"), Some("0.2.9".into()));
        // The tag form a user is likely to paste.
        assert_eq!(parse_version("v0.2.7"), Some("0.2.7".into()));
        assert_eq!(parse_version("dcw updated."), None);
    }

    // The file is the only source of these strings, so assert the shape update() relies
    // on: a newest version, and a body that is not empty.
    #[test]
    fn latest_entry_is_well_formed() {
        let (version, body) = latest().expect("CHANGELOG.md has at least one section");
        assert_eq!(version.split('.').count(), 3, "version should be x.y.z: {version}");
        assert!(!body.is_empty(), "release {version} has an empty body");
        assert!(!body.starts_with("## "), "body should not swallow the next heading");
    }

    // update() shows everything between the version it ran and the version it installed.
    #[test]
    fn since_stops_at_the_running_version() {
        let (newest, _) = latest().unwrap();
        assert_eq!(since(&newest), Some(String::new()), "same version = no notes");
        // An unknown version must be None, not a guess about the range.
        assert_eq!(since("99.99.99"), None);
    }
}