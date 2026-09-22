use crate::rules::{parse_rule, NormalizedRule};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

pub const MAX_SOURCES: usize = 2;
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SOURCE_LINES: usize = 300_000;
pub const MAX_USER_HOSTS: usize = 10_000;
pub const MAX_DIAGNOSTICS: usize = 200;
pub const MAX_SUPPRESSION_REASONS: usize = 20;
const MAX_SOURCE_NAME_BYTES: usize = 128;
const MAX_DIAGNOSTIC_RULE_BYTES: usize = 256;

#[derive(Clone, Debug)]
pub struct RuleSource {
    pub name: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub blocked: bool,
    pub source: Option<String>,
    pub rule: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceReport {
    pub name: String,
    pub lines: usize,
    pub blocks: usize,
    pub allows: usize,
    pub conservative_allows: usize,
    pub badfilters: usize,
    pub ignored: usize,
    pub unsupported: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileDiagnostic {
    pub source: String,
    pub line: usize,
    pub rule: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileReport {
    pub sources: Vec<SourceReport>,
    pub user_allow_rules: usize,
    pub user_block_rules: usize,
    pub list_allow_rules: usize,
    pub list_block_rules: usize,
    pub effective_list_block_rules: usize,
    pub list_blocks_suppressed: bool,
    pub suppression_reasons: Vec<CompileDiagnostic>,
    pub suppression_reasons_total: usize,
    pub diagnostics: Vec<CompileDiagnostic>,
    pub diagnostics_total: usize,
    pub diagnostics_omitted: usize,
}

#[derive(Clone, Debug)]
struct MatchedRule {
    source: Arc<str>,
    rule: Arc<str>,
}

type HostRules = HashMap<String, MatchedRule>;

#[derive(Clone, Debug)]
pub struct DnsPolicy {
    user_allow: HostRules,
    user_block: HostRules,
    list_allow: HostRules,
    list_block: HostRules,
    report: CompileReport,
}

pub fn normalize_hostname(hostname: &str) -> Result<String, String> {
    if hostname.is_empty() || hostname.len() > 1024 || hostname.chars().any(char::is_whitespace) {
        return Err("A hostname must be nonempty, bounded, and contain no whitespace".into());
    }
    let hostname = hostname
        .strip_suffix('.')
        .or_else(|| hostname.strip_suffix('\u{3002}'))
        .or_else(|| hostname.strip_suffix('\u{ff0e}'))
        .or_else(|| hostname.strip_suffix('\u{ff61}'))
        .unwrap_or(hostname);
    let ascii = idna::domain_to_ascii_strict(hostname)
        .map_err(|_| "A hostname must be a valid IDNA domain name".to_string())?;
    let host = ascii
        .strip_suffix('.')
        .unwrap_or(&ascii)
        .to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err("A hostname must contain valid labels of at most 63 bytes".into());
    }
    if host.parse::<IpAddr>().is_ok()
        || host
            .split('.')
            .all(|label| label.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("IP addresses are not hostname rules".into());
    }
    Ok(host)
}

impl DnsPolicy {
    pub fn compile(
        sources: &[RuleSource],
        user_allow: &[String],
        user_block: &[String],
    ) -> Result<Self, String> {
        if sources.len() > MAX_SOURCES {
            return Err(format!(
                "At most {MAX_SOURCES} DNS filter sources are supported"
            ));
        }
        for source in sources {
            if source.name.trim().is_empty()
                || source.name.len() > MAX_SOURCE_NAME_BYTES
                || source.name.chars().any(char::is_control)
            {
                return Err(format!("Source names must contain 1–{MAX_SOURCE_NAME_BYTES} bytes without control characters"));
            }
            if source.text.len() > MAX_SOURCE_BYTES {
                return Err(format!(
                    "Source {} exceeds the {MAX_SOURCE_BYTES}-byte limit",
                    source.name
                ));
            }
            if source.text.lines().take(MAX_SOURCE_LINES + 1).count() > MAX_SOURCE_LINES {
                return Err(format!(
                    "Source {} exceeds the {MAX_SOURCE_LINES}-line limit",
                    source.name
                ));
            }
        }
        let mut policy = Self {
            user_allow: compile_user_hosts(user_allow, "user allow")?,
            user_block: compile_user_hosts(user_block, "user block")?,
            list_allow: HashMap::new(),
            list_block: HashMap::new(),
            report: CompileReport::default(),
        };
        for source in sources {
            let name: Arc<str> = Arc::from(source.name.as_str());
            let mut report = SourceReport {
                name: source.name.clone(),
                ..SourceReport::default()
            };
            for (index, raw) in source.text.lines().enumerate() {
                report.lines += 1;
                let raw = raw.trim().trim_start_matches('\u{feff}').trim();
                if raw.starts_with("!#") {
                    report.unsupported += 1;
                    let message = "Unsupported preprocessing directive suppresses all list blocks; user rules still apply";
                    policy.suppress_list_blocks(&source.name, index + 1, raw, message);
                    policy.diagnostic(&source.name, index + 1, raw, message);
                    continue;
                }
                if is_ignored(raw) || cosmetic_only_exception(raw) {
                    report.ignored += 1;
                    continue;
                }
                let exception = raw.starts_with("@@");
                let badfilter = has_badfilter(raw);
                if badfilter {
                    report.badfilters += 1;
                }
                if !badfilter && raw.len() <= crate::rules::MAX_LINE_BYTES {
                    if let Some((host, exception)) = unconditional_rule(raw) {
                        let target = if exception {
                            report.allows += 1;
                            &mut policy.list_allow
                        } else {
                            report.blocks += 1;
                            &mut policy.list_block
                        };
                        target.entry(host).or_insert_with(|| MatchedRule {
                            source: Arc::clone(&name),
                            rule: Arc::from(raw),
                        });
                        continue;
                    }
                }
                report.unsupported += 1;
                let message = if exception || badfilter {
                    if page_level_exception(raw) {
                        let message = "Page-wide network exceptions suppress all list blocks because DNS has no page context";
                        policy.suppress_list_blocks(&source.name, index + 1, raw, message);
                        message
                    } else if let Some(host) = exception_suffix(raw) {
                        policy
                            .list_allow
                            .entry(host)
                            .or_insert_with(|| MatchedRule {
                                source: Arc::clone(&name),
                                rule: Arc::from(truncate_utf8(raw, MAX_DIAGNOSTIC_RULE_BYTES)),
                            });
                        report.conservative_allows += 1;
                        "Unsupported exception or badfilter protects its entire hostname suffix"
                    } else {
                        let message = "Unscopable exception or badfilter suppresses all list blocks; user rules still apply";
                        policy.suppress_list_blocks(&source.name, index + 1, raw, message);
                        message
                    }
                } else {
                    "Only unconditional ||hostname^ rules can block DNS; this rule was omitted"
                };
                policy.diagnostic(&source.name, index + 1, raw, message);
            }
            policy.report.sources.push(report);
        }
        policy.report.user_allow_rules = policy.user_allow.len();
        policy.report.user_block_rules = policy.user_block.len();
        policy.report.list_allow_rules = policy.list_allow.len();
        policy.report.list_block_rules = policy.list_block.len();
        policy.report.effective_list_block_rules = if policy.report.list_blocks_suppressed {
            0
        } else {
            policy.list_block.len()
        };
        policy.report.diagnostics_omitted =
            policy.report.diagnostics_total - policy.report.diagnostics.len();
        Ok(policy)
    }

    pub fn decide(&self, hostname: &str) -> Decision {
        let Ok(host) = normalize_hostname(hostname) else {
            return Decision {
                blocked: false,
                source: None,
                rule: None,
            };
        };
        for (rules, blocked) in [
            (&self.user_allow, false),
            (&self.user_block, true),
            (&self.list_allow, false),
        ] {
            if let Some(rule) = suffix_match(rules, &host) {
                return decision(blocked, rule);
            }
        }
        if !self.report.list_blocks_suppressed {
            if let Some(rule) = suffix_match(&self.list_block, &host) {
                return decision(true, rule);
            }
        }
        Decision {
            blocked: false,
            source: None,
            rule: None,
        }
    }

    pub fn report(&self) -> &CompileReport {
        &self.report
    }

    fn suppress_list_blocks(&mut self, source: &str, line: usize, rule: &str, message: &str) {
        self.report.list_blocks_suppressed = true;
        self.report.suppression_reasons_total += 1;
        if self.report.suppression_reasons.len() < MAX_SUPPRESSION_REASONS {
            self.report.suppression_reasons.push(CompileDiagnostic {
                source: source.to_string(),
                line,
                rule: truncate_utf8(rule, MAX_DIAGNOSTIC_RULE_BYTES).to_string(),
                message: truncate_utf8(message, MAX_DIAGNOSTIC_RULE_BYTES).to_string(),
            });
        }
    }

    fn diagnostic(&mut self, source: &str, line: usize, rule: &str, message: &str) {
        self.report.diagnostics_total += 1;
        if self.report.diagnostics.len() < MAX_DIAGNOSTICS {
            self.report.diagnostics.push(CompileDiagnostic {
                source: source.to_string(),
                line,
                rule: truncate_utf8(rule, MAX_DIAGNOSTIC_RULE_BYTES).to_string(),
                message: message.to_string(),
            });
        }
    }
}

fn compile_user_hosts(hosts: &[String], source: &str) -> Result<HostRules, String> {
    if hosts.len() > MAX_USER_HOSTS {
        return Err(format!(
            "{source} accepts at most {MAX_USER_HOSTS} hostnames"
        ));
    }
    let source: Arc<str> = Arc::from(source);
    let mut rules = HashMap::new();
    for hostname in hosts {
        let host = normalize_hostname(hostname)?;
        rules.entry(host.clone()).or_insert_with(|| MatchedRule {
            source: Arc::clone(&source),
            rule: Arc::from(host),
        });
    }
    Ok(rules)
}

fn unconditional_rule(raw: &str) -> Option<(String, bool)> {
    let (prefix, body) = raw
        .strip_prefix("@@")
        .map_or(("", raw), |body| ("@@", body));
    let host = body.strip_prefix("||")?.strip_suffix('^')?;
    let host = normalize_hostname(host).ok()?;
    let canonical = format!("{prefix}||{host}^");
    match parse_rule(&canonical).ok()? {
        NormalizedRule::Network {
            exception,
            third_party: false,
            ..
        } => Some((host, exception)),
        _ => None,
    }
}

fn exception_suffix(raw: &str) -> Option<String> {
    let body = raw.strip_prefix("@@").unwrap_or(raw);
    let pattern = body.split('$').next()?;
    let anchored = pattern.strip_prefix("||")?;
    let boundary = anchored.find(['^', '/', ':', '|'])?;
    normalize_hostname(&anchored[..boundary]).ok()
}

fn has_badfilter(raw: &str) -> bool {
    raw.split_once('$').is_some_and(|(_, modifiers)| {
        modifiers
            .split(',')
            .any(|modifier| modifier.trim().eq_ignore_ascii_case("badfilter"))
    })
}

fn cosmetic_only_exception(raw: &str) -> bool {
    if !raw.starts_with("@@") {
        return false;
    }
    let Some((_, options)) = raw.split_once('$') else {
        return false;
    };
    let cosmetic = [
        "generichide",
        "elemhide",
        "specifichide",
        "ghide",
        "ehide",
        "shide",
    ];
    options.split(',').any(|option| cosmetic.contains(&option))
        && options.split(',').all(|option| {
            cosmetic.contains(&option)
                || option.starts_with("domain=")
                || [
                    "third-party",
                    "~third-party",
                    "first-party",
                    "~first-party",
                    "match-case",
                    "badfilter",
                ]
                .contains(&option)
        })
}

fn page_level_exception(raw: &str) -> bool {
    raw.starts_with("@@")
        && raw.split_once('$').is_some_and(|(_, options)| {
            options
                .split(',')
                .any(|option| ["document", "doc", "genericblock"].contains(&option))
        })
}

fn is_ignored(raw: &str) -> bool {
    raw.is_empty()
        || raw.starts_with('!')
        || raw.starts_with('[')
        || (!raw.starts_with("@@")
            && ["##", "#@#", "#?#", "#@?#", "#$#", "#@$#", "#%#", "#@%#"]
                .iter()
                .any(|marker| raw.contains(marker)))
}

fn suffix_match<'a>(rules: &'a HostRules, hostname: &str) -> Option<&'a MatchedRule> {
    let mut suffix = hostname;
    loop {
        if let Some(rule) = rules.get(suffix) {
            return Some(rule);
        }
        suffix = suffix.split_once('.')?.1;
    }
}

fn decision(blocked: bool, matched: &MatchedRule) -> Decision {
    Decision {
        blocked,
        source: Some(matched.source.to_string()),
        rule: Some(matched.rule.to_string()),
    }
}

fn truncate_utf8(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(text: &str) -> DnsPolicy {
        DnsPolicy::compile(
            &[RuleSource {
                name: "test".into(),
                text: text.into(),
            }],
            &[],
            &[],
        )
        .unwrap()
    }

    #[test]
    fn normalizes_idna_case_and_root_dot() {
        assert_eq!(
            normalize_hostname("WWW.Example.COM.").unwrap(),
            "www.example.com"
        );
        assert_eq!(
            normalize_hostname("BÜCHER.example").unwrap(),
            "xn--bcher-kva.example"
        );
        let policy = compile("||BÜCHER.example.^");
        assert!(policy.decide("sub.xn--bcher-kva.example").blocked);
    }

    #[test]
    fn invalid_names_and_addresses_cannot_be_rules() {
        for name in [
            "",
            ".",
            "a..b",
            "-a.test",
            "a-.test",
            "a_b.test",
            "a.test..",
            " test",
            "127.0.0.1",
            "127.1",
            "::1",
            "[::1]",
            "example.com:80",
            "http://a.test",
        ] {
            assert!(normalize_hostname(name).is_err(), "{name}");
        }
        assert!(normalize_hostname(&format!("{}.test", "a".repeat(64))).is_err());
        assert!(DnsPolicy::compile(&[], &[], &["127.0.0.1".into()]).is_err());
    }

    #[test]
    fn blocks_host_and_subdomains_without_partial_label_matches() {
        let policy = compile("||ads.example^");
        for name in ["ads.example", "sub.ads.example", "ADS.EXAMPLE."] {
            assert!(policy.decide(name).blocked);
        }
        for name in ["notads.example", "ads.example.other", "example"] {
            assert!(!policy.decide(name).blocked);
        }
    }

    #[test]
    fn never_broadens_paths_or_contextual_blocks_into_dns() {
        let policy = compile("||path.example/ads.js\n||third.example^$third-party\n||image.example^$image\n||context.example^$domain=site.example\n/ads/\n0.0.0.0 hosts.example\nsite.example##.ad\nsite.example#@#.ad");
        assert_eq!(policy.report().list_block_rules, 0);
        assert_eq!(policy.report().sources[0].unsupported, 6);
        assert_eq!(policy.report().sources[0].ignored, 2);
    }

    #[test]
    fn unsupported_exceptions_protect_the_whole_identifiable_suffix() {
        for exception in [
            "@@||ads.example/allowed.js",
            "@@||ads.example^$image",
            "@@||ads.example^$domain=site.example",
            "@@||ADS.EXAMPLE.:443/path",
        ] {
            let policy = compile(&format!("||ads.example^\n||other.example^\n{exception}"));
            assert!(!policy.decide("ads.example").blocked, "{exception}");
            assert!(!policy.decide("sub.ads.example").blocked, "{exception}");
            assert!(policy.decide("other.example").blocked, "{exception}");
            assert!(!policy.report().list_blocks_suppressed, "{exception}");
        }
    }

    #[test]
    fn unscopable_exceptions_suppress_all_sources_but_not_user_overrides() {
        let sources = [
            RuleSource {
                name: "one".into(),
                text: "||ads.example^\n@@/allowed/".into(),
            },
            RuleSource {
                name: "two".into(),
                text: "||other.example^".into(),
            },
        ];
        let policy = DnsPolicy::compile(
            &sources,
            &["safe.user.example".into()],
            &["user.example".into()],
        )
        .unwrap();
        assert!(policy.report().list_blocks_suppressed);
        assert!(!policy.decide("ads.example").blocked);
        assert!(!policy.decide("other.example").blocked);
        assert!(policy.decide("user.example").blocked);
        assert!(!policy.decide("safe.user.example").blocked);
        for exception in [
            "@@||ads.example",
            "@@||ads.*^",
            "@@||*.example^",
            "@@https://ads.example/path",
        ] {
            assert!(
                compile(exception).report().list_blocks_suppressed,
                "{exception}"
            );
        }
    }

    #[test]
    fn badfilter_cancellations_cannot_leave_broadened_blocks_active() {
        let policy = compile("||ads.example^\n||ads.example^$badfilter\n||other.example^\n@@||other.example^\n@@||other.example^$badfilter");
        assert!(!policy.decide("ads.example").blocked);
        assert!(!policy.decide("other.example").blocked);
        assert_eq!(policy.report().sources[0].badfilters, 2);
        assert!(
            compile("||ads.example^\n/ads/$badfilter")
                .report()
                .list_blocks_suppressed
        );
    }

    #[test]
    fn cosmetic_only_exceptions_do_not_disable_network_filtering() {
        let policy = compile("||ads.example^\n@@*$generichide\n@@||site.example^$elemhide,domain=other.example\n@@*$shide\n@@*$generichide,badfilter");
        assert!(policy.decide("ads.example").blocked);
        assert!(!policy.report().list_blocks_suppressed);
        assert_eq!(policy.report().sources[0].ignored, 4);
        assert!(
            compile("||ads.example^\n@@*$generichide,script")
                .report()
                .list_blocks_suppressed
        );
    }

    #[test]
    fn page_level_exemptions_protect_requests_to_other_domains() {
        for option in ["document", "doc", "genericblock", "document,generichide"] {
            let policy = compile(&format!("||ads.example^\n@@||site.example^${option}"));
            assert!(policy.report().list_blocks_suppressed, "{option}");
            assert!(!policy.decide("ads.example").blocked, "{option}");
        }
    }

    #[test]
    fn precedence_is_user_allow_user_block_list_allow_list_block() {
        let policy = DnsPolicy::compile(
            &[RuleSource {
                name: "list".into(),
                text: "||example^\n@@||safe.example^\n||deep.safe.example^".into(),
            }],
            &["allowed.safe.example".into()],
            &["safe.example".into()],
        )
        .unwrap();
        assert!(!policy.decide("child.allowed.safe.example").blocked);
        assert!(policy.decide("safe.example").blocked);
        assert_eq!(
            policy.decide("safe.example").source.as_deref(),
            Some("user block")
        );
        let list_only = compile("||example^\n@@||safe.example^\n||deep.safe.example^");
        assert!(!list_only.decide("deep.safe.example").blocked);
        assert!(list_only.decide("other.example").blocked);
    }

    #[test]
    fn returns_most_specific_rule_within_a_precedence_tier() {
        let policy = compile("||example^\n||ads.example^\n||ads.example^");
        let decision = policy.decide("sub.ads.example");
        assert_eq!(decision.source.as_deref(), Some("test"));
        assert_eq!(decision.rule.as_deref(), Some("||ads.example^"));
        assert_eq!(policy.report().list_block_rules, 2);
    }

    #[test]
    fn diagnostics_and_rule_text_are_bounded_with_complete_totals() {
        let text = format!("{}\n", "ü".repeat(2000)).repeat(MAX_DIAGNOSTICS + 5);
        let policy = compile(&text);
        assert_eq!(policy.report().diagnostics.len(), MAX_DIAGNOSTICS);
        assert_eq!(policy.report().diagnostics_total, MAX_DIAGNOSTICS + 5);
        assert_eq!(policy.report().diagnostics_omitted, 5);
        assert!(policy
            .report()
            .diagnostics
            .iter()
            .all(|item| item.rule.len() <= MAX_DIAGNOSTIC_RULE_BYTES));
    }

    #[test]
    fn rejects_oversized_sources_names_line_counts_and_overrides() {
        let source = RuleSource {
            name: "test".into(),
            text: String::new(),
        };
        assert!(DnsPolicy::compile(&vec![source.clone(); MAX_SOURCES + 1], &[], &[]).is_err());
        for name in [
            "".to_string(),
            " ".to_string(),
            "name\n".to_string(),
            "x".repeat(MAX_SOURCE_NAME_BYTES + 1),
        ] {
            assert!(DnsPolicy::compile(
                &[RuleSource {
                    name,
                    text: String::new()
                }],
                &[],
                &[]
            )
            .is_err());
        }
        assert!(DnsPolicy::compile(
            &[RuleSource {
                name: "test".into(),
                text: "!".repeat(MAX_SOURCE_BYTES + 1)
            }],
            &[],
            &[]
        )
        .is_err());
        assert!(DnsPolicy::compile(
            &[RuleSource {
                name: "test".into(),
                text: "\n".repeat(MAX_SOURCE_LINES + 1)
            }],
            &[],
            &[]
        )
        .is_err());
        assert!(DnsPolicy::compile(&[], &vec!["example".into(); MAX_USER_HOSTS + 1], &[]).is_err());
    }

    #[test]
    fn oversized_exception_retains_safety_even_when_diagnostics_are_full() {
        let text = format!(
            "{}@@||ads.example^$domain={}\n@@/unscopable/",
            "unsupported\n".repeat(MAX_DIAGNOSTICS),
            "a".repeat(3000)
        );
        let policy = compile(&text);
        assert!(policy.report().list_blocks_suppressed);
        assert_eq!(policy.report().list_allow_rules, 1);
        assert_eq!(policy.report().diagnostics_total, MAX_DIAGNOSTICS + 2);
    }

    #[test]
    fn suppression_reasons_survive_diagnostic_truncation_and_remain_bounded() {
        let policy = compile(&format!(
            "{}||ads.example^\n{}",
            "unsupported\n".repeat(MAX_DIAGNOSTICS),
            "!#include exceptions.txt\n".repeat(MAX_SUPPRESSION_REASONS + 3),
        ));
        let report = policy.report();
        assert_eq!(report.list_block_rules, 1);
        assert_eq!(report.effective_list_block_rules, 0);
        assert_eq!(report.suppression_reasons.len(), MAX_SUPPRESSION_REASONS);
        assert_eq!(
            report.suppression_reasons_total,
            MAX_SUPPRESSION_REASONS + 3
        );
        assert_eq!(report.suppression_reasons[0].source, "test");
        assert_eq!(report.suppression_reasons[0].line, MAX_DIAGNOSTICS + 2);
        assert_eq!(
            report.suppression_reasons[0].rule,
            "!#include exceptions.txt"
        );
        assert!(report.suppression_reasons[0]
            .message
            .contains("preprocessing directive"));
        assert!(report
            .suppression_reasons
            .iter()
            .all(|reason| reason.message.len() <= 256));
        assert_eq!(
            compile("||ads.example^")
                .report()
                .effective_list_block_rules,
            1
        );
        for exception in ["@@/unscopable/", "@@||site.example^$document"] {
            let policy = compile(&format!(
                "{}\n{exception}",
                "unsupported\n".repeat(MAX_DIAGNOSTICS)
            ));
            assert_eq!(policy.report().suppression_reasons_total, 1);
            assert_eq!(policy.report().suppression_reasons[0].rule, exception);
        }
    }

    #[test]
    fn preprocessing_directives_suppress_list_blocks_and_preserve_user_overrides() {
        for directive in [
            "!#if false",
            "!#else",
            "!#endif",
            "!#include other.txt",
            "!#unknown",
        ] {
            let policy = DnsPolicy::compile(
                &[
                    RuleSource {
                        name: "conditional".into(),
                        text: format!("{directive}\n||ads.example^"),
                    },
                    RuleSource {
                        name: "other".into(),
                        text: "||other.example^".into(),
                    },
                ],
                &["safe.user.example".into()],
                &["user.example".into()],
            )
            .unwrap();
            assert!(policy.report().list_blocks_suppressed, "{directive}");
            assert!(!policy.decide("ads.example").blocked, "{directive}");
            assert!(!policy.decide("other.example").blocked, "{directive}");
            assert!(policy.decide("user.example").blocked, "{directive}");
            assert!(!policy.decide("safe.user.example").blocked, "{directive}");
            assert_eq!(policy.report().sources[0].unsupported, 1);
            assert_eq!(policy.report().diagnostics_total, 1);
            assert!(policy.report().diagnostics[0]
                .message
                .contains("preprocessing directive"));
        }
        let policy = compile("!#if false\n||ads.example^\n!#endif");
        assert!(!policy.decide("ads.example").blocked);
        assert_eq!(policy.report().diagnostics_total, 2);
        assert!(
            compile("! ordinary comment\n||ads.example^")
                .decide("ads.example")
                .blocked
        );
        assert!(
            compile(&format!(
                "{}!#include exceptions.txt",
                "unsupported\n".repeat(MAX_DIAGNOSTICS)
            ))
            .report()
            .list_blocks_suppressed
        );
    }
}
