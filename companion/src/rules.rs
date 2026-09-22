//! Intentionally conservative Adblock-style subset. Unsupported syntax never
//! falls back to a broader filter. This is not a full EasyList/uBO parser.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_TEXT_BYTES: usize = 128 * 1024;
pub const MAX_NONEMPTY_LINES: usize = 2_000;
pub const MAX_LINE_BYTES: usize = 2_048;
pub const MAX_SELECTOR_BYTES: usize = 512;
pub const MAX_COSMETIC_DOMAINS: usize = 200;
pub const MAX_NETWORK_ID: u32 = 999_999_999;

// Main-frame navigation is deliberately outside this first build's coverage.
pub const RESOURCE_TYPES: [&str; 11] = [
    "sub_frame",
    "stylesheet",
    "script",
    "image",
    "font",
    "object",
    "xmlhttprequest",
    "ping",
    "media",
    "websocket",
    "other",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedRule {
    Network {
        pattern: String,
        exception: bool,
        third_party: bool,
    },
    Cosmetic {
        domains: Vec<String>,
        selector: String,
    },
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Target {
    Mv3Network,
    Cosmetic,
    Unsupported,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Compilation {
    pub network_rules: Vec<NetworkRule>,
    pub cosmetic_rules: Vec<CosmeticRule>,
    pub diagnostics: Vec<Diagnostic>,
    pub stats: Stats,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NetworkRule {
    pub id: u32,
    pub priority: u32,
    pub action: Action,
    pub condition: Condition,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Action {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub url_filter: String,
    pub resource_types: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_type: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct CosmeticRule {
    pub domains: Vec<String>,
    pub selector: String,
    pub raw: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub line: usize,
    pub raw: String,
    pub target: Target,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub network: usize,
    pub cosmetic: usize,
    pub unsupported: usize,
    pub ignored: usize,
}

/// Parses one non-comment rule into a backend-independent supported model.
pub fn parse_rule(raw: &str) -> Result<NormalizedRule, &'static str> {
    if raw.contains("#@#") {
        return Err("Cosmetic exceptions are not supported; no rule was emitted");
    }
    if let Some((scope, selector)) = raw.split_once("##") {
        if selector.len() > MAX_SELECTOR_BYTES {
            return Err("Cosmetic selector exceeds the 512-byte limit; no rule was emitted");
        }
        if !valid_selector(selector) {
            return Err("Only compound tag/class/ID selectors and one :has(> .class-or-ID) child check are supported");
        }
        let domains = if scope.is_empty() {
            vec![]
        } else {
            let mut domains = BTreeSet::new();
            for domain in scope.split(',') {
                if !valid_hostname(domain) {
                    return Err("Cosmetic scope must be comma-separated plain ASCII hostnames; negations and wildcards are unsupported");
                }
                domains.insert(domain.to_ascii_lowercase());
                if domains.len() > MAX_COSMETIC_DOMAINS {
                    return Err("Cosmetic scope exceeds the 200-domain limit; no rule was emitted");
                }
            }
            domains.into_iter().collect()
        };
        return Ok(NormalizedRule::Cosmetic {
            domains,
            selector: canonical_selector(selector),
        });
    }

    let (exception, body) = match raw.strip_prefix("@@") {
        Some(body) => (true, body),
        None => (false, raw),
    };
    let (pattern, third_party) = if let Some((pattern, modifiers)) = body.split_once('$') {
        if modifiers != "third-party" {
            return Err("Only the $third-party network modifier is supported; no rule was emitted");
        }
        (pattern, true)
    } else {
        (body, false)
    };
    let Some(anchored) = pattern.strip_prefix("||") else {
        return Err("Only domain-anchored network patterns beginning with || are supported");
    };
    let (host, suffix) = if let Some(host) = anchored.strip_suffix('^') {
        (host, "^")
    } else if let Some(slash) = anchored.find('/') {
        let (host, path) = anchored.split_at(slash);
        if !valid_literal_path(path) {
            return Err("Network paths must be literal ASCII URL paths; wildcards, queries, fragments, and anchors are unsupported");
        }
        (host, path)
    } else {
        return Err("A hostname must end with ^ or a literal /path to avoid partial-host matches");
    };
    if !valid_hostname(host) {
        return Err("Network patterns require a valid plain ASCII hostname; regexes, wildcards, ports, and Unicode are unsupported");
    }
    Ok(NormalizedRule::Network {
        pattern: format!("||{}{suffix}", host.to_ascii_lowercase()),
        exception,
        third_party,
    })
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_literal_path(path: &str) -> bool {
    path.starts_with('/')
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._%~:@!&=+-".contains(&byte))
}

/// Compounds, optionally followed by one direct-child :has() check. The child
/// must have a class or ID; nesting, lists, escapes and other CSS are rejected.
pub fn valid_selector(selector: &str) -> bool {
    if selector.is_empty() || selector.len() > MAX_SELECTOR_BYTES {
        return false;
    }
    if let Some((parent, tail)) = selector.split_once(":has(>") {
        let Some(child) = tail.strip_suffix(')') else {
            return false;
        };
        let child = child.trim_matches(' ');
        return valid_compound(parent) && child.contains(['.', '#']) && valid_compound(child);
    }
    valid_compound(selector)
}

fn valid_compound(selector: &str) -> bool {
    let bytes = selector.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_SELECTOR_BYTES {
        return false;
    }
    let mut index = 0;
    if bytes[0].is_ascii_alphabetic() {
        index += 1;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'-')
        {
            index += 1;
        }
    }
    while index < bytes.len() {
        if bytes[index] != b'.' && bytes[index] != b'#' {
            return false;
        }
        index += 1;
        if index == bytes.len() {
            return false;
        }
        // CSS identifiers may start with letters, underscore, or a hyphen when
        // followed by a letter/underscore/hyphen; digit-leading IDs need escapes.
        if bytes[index] == b'-' {
            index += 1;
            if index == bytes.len()
                || !(bytes[index].is_ascii_alphabetic()
                    || bytes[index] == b'_'
                    || bytes[index] == b'-')
            {
                return false;
            }
        } else if !(bytes[index].is_ascii_alphabetic() || bytes[index] == b'_') {
            return false;
        }
        index += 1;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'_'
                || bytes[index] == b'-')
        {
            index += 1;
        }
    }
    index == bytes.len()
}

/// Called after validation. Equivalent child-check spacing must have the same
/// key so cosmetic exceptions and deduplication cannot disagree with CSS.
pub fn canonical_selector(selector: &str) -> String {
    if let Some((parent, tail)) = selector.split_once(":has(>") {
        if let Some(child) = tail.strip_suffix(')') {
            return format!("{parent}:has(>{})", child.trim_matches(' '));
        }
    }
    selector.into()
}

fn key_for(rule: &NormalizedRule) -> String {
    match rule {
        NormalizedRule::Network {
            pattern,
            exception,
            third_party,
        } => format!("{exception}|{third_party}|{pattern}"),
        NormalizedRule::Cosmetic { domains, selector } => {
            format!("{}##{selector}", domains.join(","))
        }
    }
}

/// FNV-1a is used only for reproducible identifiers, never as a security hash.
fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn assign_ids<'a>(
    keys: impl IntoIterator<Item = &'a String>,
    hash: impl Fn(&str) -> u64,
) -> BTreeMap<String, u32> {
    // Sorted canonical input makes collision resolution independent of list
    // ordering. Changing the set can reassign colliding IDs; updates are atomic.
    let mut taken = BTreeSet::new();
    let mut ids = BTreeMap::new();
    for key in keys {
        let mut id = (hash(key) % u64::from(MAX_NETWORK_ID)) as u32 + 1;
        while !taken.insert(id) {
            id = if id == MAX_NETWORK_ID { 1 } else { id + 1 };
        }
        ids.insert(key.clone(), id);
    }
    ids
}

pub fn compile(text: &str, source: &str) -> Result<Compilation, &'static str> {
    if text.len() > MAX_TEXT_BYTES {
        return Err("Filter text exceeds the 128 KiB limit");
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut nonempty = 0;
    // Validate every bound before producing any rules: limit failures reject the
    // entire import rather than replacing protection with a partial compilation.
    for line in text.lines() {
        if line.len() > MAX_LINE_BYTES {
            return Err("A filter line exceeds the 2,048-byte limit");
        }
        if !line.trim().is_empty() {
            nonempty += 1;
        }
        if nonempty > MAX_NONEMPTY_LINES {
            return Err("Filter list exceeds the 2,000 nonempty-line limit");
        }
    }

    let mut result = Compilation {
        network_rules: vec![],
        cosmetic_rules: vec![],
        diagnostics: vec![],
        stats: Stats::default(),
    };
    let mut network = BTreeMap::new();
    let mut cosmetic = BTreeSet::new();
    for (index, original) in text.lines().enumerate() {
        let raw = original.trim();
        if raw.is_empty()
            || raw.starts_with('!')
            || raw == "[Adblock]"
            || raw == "[Adblock Plus 2.0]"
        {
            result.stats.ignored += 1;
            continue;
        }
        match parse_rule(raw) {
            Ok(rule @ NormalizedRule::Network { .. }) => {
                let duplicate = network.insert(key_for(&rule), rule).is_some();
                result.diagnostics.push(Diagnostic {
                    line: index + 1,
                    raw: raw.into(),
                    target: Target::Mv3Network,
                    message: if duplicate {
                        "Duplicate supported rule; emitted once"
                    } else {
                        "Compiled successfully"
                    }
                    .into(),
                });
            }
            Ok(rule @ NormalizedRule::Cosmetic { .. }) => {
                let duplicate = !cosmetic.insert(key_for(&rule));
                if !duplicate {
                    if let NormalizedRule::Cosmetic { domains, selector } = rule {
                        result.cosmetic_rules.push(CosmeticRule {
                            domains,
                            selector,
                            raw: raw.into(),
                            source: source.into(),
                        });
                    }
                }
                result.diagnostics.push(Diagnostic {
                    line: index + 1,
                    raw: raw.into(),
                    target: Target::Cosmetic,
                    message: if duplicate {
                        "Duplicate supported rule; emitted once"
                    } else {
                        "Compiled successfully"
                    }
                    .into(),
                });
            }
            Err(message) => {
                result.stats.unsupported += 1;
                result.diagnostics.push(Diagnostic {
                    line: index + 1,
                    raw: raw.into(),
                    target: Target::Unsupported,
                    message: message.into(),
                });
            }
        }
    }
    let ids = assign_ids(network.keys(), stable_hash);
    for (key, rule) in network {
        if let NormalizedRule::Network {
            pattern,
            exception,
            third_party,
        } = rule
        {
            result.network_rules.push(NetworkRule {
                id: ids[&key],
                priority: if exception { 2 } else { 1 },
                action: Action {
                    kind: if exception { "allow" } else { "block" },
                },
                condition: Condition {
                    url_filter: pattern,
                    resource_types: RESOURCE_TYPES.to_vec(),
                    domain_type: third_party.then_some("thirdParty"),
                },
            });
        }
    }
    result.stats.network = result.network_rules.len();
    result.stats.cosmetic = result.cosmetic_rules.len();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roadmap_examples_compile_to_expected_targets() {
        let result = compile("||example.com^\n||ads.example.com^$third-party\n@@||example.com/resource.js\nexample.com##.advertisement\n##.generic-ad-class", "Fixture").unwrap();
        assert_eq!(
            (
                result.stats.network,
                result.stats.cosmetic,
                result.stats.unsupported
            ),
            (3, 2, 0)
        );
        let third_party = result
            .network_rules
            .iter()
            .find(|rule| rule.condition.domain_type.is_some())
            .unwrap();
        assert_eq!(third_party.condition.domain_type, Some("thirdParty"));
        assert_eq!(third_party.condition.url_filter, "||ads.example.com^");
        assert_eq!(result.cosmetic_rules[0].domains, vec!["example.com"]);
        assert!(result.cosmetic_rules[1].domains.is_empty());
        assert_eq!(result.cosmetic_rules[0].source, "Fixture");
    }

    #[test]
    fn exception_priority_exceeds_blocks_and_navigation_is_excluded() {
        let result = compile("||example.test^\n@@||example.test/resource.js", "test").unwrap();
        let allow = result
            .network_rules
            .iter()
            .find(|rule| rule.action.kind == "allow")
            .unwrap();
        let block = result
            .network_rules
            .iter()
            .find(|rule| rule.action.kind == "block")
            .unwrap();
        assert_eq!((allow.priority, block.priority), (2, 1));
        for rule in &result.network_rules {
            assert_eq!(rule.condition.resource_types, RESOURCE_TYPES);
            assert!(!rule.condition.resource_types.contains(&"main_frame"));
        }
    }

    #[test]
    fn deterministic_ids_deduplicate_and_survive_reordering() {
        let a = compile(
            "||B.example^\n||a.example^\n||b.example^\n@@||a.example^",
            "a",
        )
        .unwrap();
        let b = compile("@@||a.example^\n||b.example^\n||a.example^", "b").unwrap();
        assert_eq!(a.network_rules, b.network_rules);
        assert_eq!(a.stats.network, 3);
        assert_eq!(a.diagnostics.len(), 4);
        let ids: BTreeSet<_> = a.network_rules.iter().map(|rule| rule.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| *id > 0 && *id < 1_000_000_000));
    }

    #[test]
    fn forced_hash_collisions_probe_and_wrap_without_duplicates() {
        let keys = BTreeSet::from(["a".to_owned(), "b".to_owned(), "c".to_owned()]);
        let ids = assign_ids(&keys, |_| u64::from(MAX_NETWORK_ID - 1));
        assert_eq!(ids["a"], MAX_NETWORK_ID);
        assert_eq!(ids["b"], 1);
        assert_eq!(ids["c"], 2);
    }

    #[test]
    fn comments_headers_crlf_and_bom_are_ignored_with_correct_line_numbers() {
        let result = compile(
            "\u{feff}[Adblock Plus 2.0]\r\n! Title\r\n\r\n||EXAMPLE.test^\r\nunsupported",
            "test",
        )
        .unwrap();
        assert_eq!(result.stats.ignored, 3);
        assert_eq!(result.diagnostics[0].line, 4);
        assert_eq!(result.diagnostics[1].line, 5);
        assert_eq!(
            result.network_rules[0].condition.url_filter,
            "||example.test^"
        );
    }

    #[test]
    fn unsupported_network_syntax_never_produces_broader_rules() {
        let unsupported = [
            "||example.test^$script",
            "||example.test^$third-party,script",
            "||example.test^$~third-party",
            "||example.test^$domain=good.test",
            "||example.test^$redirect=noopjs",
            "||example.test^$badfilter",
            "||example.test^$important",
            "||example.test^$match-case",
            "/ads.+/",
            "ads",
            "|https://example.test/",
            "||*.example.test^",
            "||example.test",
            "||example.test/path*",
            "||example.test/a?x=1",
            "||example.test/a#part",
            "||example.test/a|",
            "||example.test/a^",
            "||example.test:80^",
            "||-bad.test^",
            "||bad..test^",
            "||example.test.^",
            "||éxample.test^",
            "||example.test^ $third-party",
            "@@||example.test^$document",
            "example.test#?#div:has(.ad)",
            "example.test#$#body{display:none}",
            "example.test##+js(set, foo, bar)",
        ];
        let result = compile(&unsupported.join("\n"), "test").unwrap();
        assert!(result.network_rules.is_empty());
        assert!(result.cosmetic_rules.is_empty());
        assert_eq!(result.stats.unsupported, unsupported.len());
        assert!(result
            .diagnostics
            .iter()
            .all(|entry| entry.target == Target::Unsupported));
    }

    #[test]
    fn cosmetic_scope_normalizes_and_deduplicates_plain_domains() {
        let result = compile(
            "EXAMPLE.test,ads.test,example.test##div.ad\nads.test,example.test##div.ad",
            "test",
        )
        .unwrap();
        assert_eq!(result.stats.cosmetic, 1);
        assert_eq!(
            result.cosmetic_rules[0].domains,
            vec!["ads.test", "example.test"]
        );
        assert_eq!(result.diagnostics.len(), 2);
        for raw in [
            "~example.test##.ad",
            "example.*##.ad",
            ",example.test##.ad",
            "example.test,##.ad",
            "example.test#@#.ad",
            "#@#.ad",
        ] {
            assert!(parse_rule(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn selectors_accept_only_safe_compounds() {
        for selector in [
            "div",
            "custom-tag",
            ".ad",
            "#sponsor",
            "div.ad.banner#slot",
            ".ad-2",
            "._private",
            ".-ad",
            ".--ad",
            "div:has(> .t-j-inbanlabel-container)",
            ".slot:has(>#ad-label)",
        ] {
            assert!(valid_selector(selector), "{selector}");
        }
        for selector in [
            "",
            "*",
            ".",
            "#",
            "div > .ad",
            "div .ad",
            "div+.ad",
            "[id=ad]",
            "div:has(.ad)",
            "div:has(> video)",
            "div:has(> .ad:has(> .nested))",
            "div:has(> .ad),body",
            "div:has(> .ad) .child",
            "div:has(> [data-ad])",
            "div:has(> .ad\n)",
            "div:has(> .ad){display:none}",
            ".ad,.banner",
            ".ad{color:red}",
            ".ad/*comment*/",
            ".123",
            ".-1",
            ".-",
            "1div",
            ".a\\31",
            ".é",
            "script\n.ad",
        ] {
            assert!(!valid_selector(selector), "{selector}");
        }
    }

    #[test]
    fn cosmetic_selector_length_matches_extension_limit() {
        let accepted = format!(".{}", "a".repeat(MAX_SELECTOR_BYTES - 1));
        let rejected = format!("{accepted}a");
        assert!(valid_selector(&accepted));
        assert!(!valid_selector(&rejected));
        let result = compile(&format!("##{accepted}\n##{rejected}"), "test").unwrap();
        assert_eq!(result.stats.cosmetic, 1);
        assert_eq!(result.stats.unsupported, 1);
        assert_eq!(result.diagnostics[1].target, Target::Unsupported);
        assert!(result.diagnostics[1].message.contains("512-byte"));
        assert_eq!(result.cosmetic_rules[0].selector, accepted);
    }

    #[test]
    fn cosmetic_domain_count_matches_extension_limit_after_normalization() {
        let domains: Vec<_> = (0..MAX_COSMETIC_DOMAINS)
            .map(|index| format!("d{index}"))
            .collect();
        let scope = domains.join(",");
        let accepted = format!("{scope}##.ad");
        let rejected = format!("{scope},one-more##.ad");
        let duplicate = format!("{scope},D0##.ad");
        let result = compile(&format!("{accepted}\n{rejected}\n{duplicate}"), "test").unwrap();
        assert_eq!(result.stats.cosmetic, 1);
        assert_eq!(result.stats.unsupported, 1);
        assert_eq!(result.cosmetic_rules[0].domains.len(), MAX_COSMETIC_DOMAINS);
        assert_eq!(result.diagnostics[1].target, Target::Unsupported);
        assert!(result.diagnostics[1].message.contains("200-domain"));
        assert!(result.diagnostics[2].message.contains("Duplicate"));
    }

    #[test]
    fn all_import_limits_are_enforced_before_any_compilation() {
        assert!(compile(&"x".repeat(MAX_TEXT_BYTES + 1), "test").is_err());
        assert!(compile(
            &format!("||valid.test^\n{}", "x".repeat(MAX_LINE_BYTES + 1)),
            "test"
        )
        .is_err());
        assert!(compile(&"!\n".repeat(MAX_NONEMPTY_LINES + 1), "test").is_err());
        assert!(compile(&"!\n".repeat(MAX_NONEMPTY_LINES), "test").is_ok());
        let line = format!("!{}", "a".repeat(MAX_LINE_BYTES - 1));
        assert!(compile(&line, "test").is_ok());
        assert!(compile(&"\n".repeat(4000), "test").is_ok());
    }

    #[test]
    fn serialized_targets_match_protocol_and_optional_conditions_are_omitted() {
        let result = compile("||example.test^\n##.ad\nunsupported", "test").unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["diagnostics"][0]["target"], "MV3_NETWORK");
        assert_eq!(value["diagnostics"][1]["target"], "COSMETIC");
        assert_eq!(value["diagnostics"][2]["target"], "UNSUPPORTED");
        assert!(value["networkRules"][0]["condition"]
            .get("domainType")
            .is_none());
    }
}
