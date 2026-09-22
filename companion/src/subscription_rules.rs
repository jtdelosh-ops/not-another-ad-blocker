//! Bounded compilation of real subscriptions. Never execute list content.
//! Unsupported exceptions weaken subscription coverage conservatively.
use crate::rules::{canonical_selector, valid_selector, RESOURCE_TYPES};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINES: usize = 300_000;
const MAX_COSMETIC: usize = 10_000;
const MAX_DIAGNOSTICS: usize = 200;

fn host_valid(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && s.as_bytes()[0].is_ascii_alphanumeric()
                && s.as_bytes()[s.len() - 1].is_ascii_alphanumeric()
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn scopes(value: &str, delimiter: char) -> Result<(Vec<String>, Vec<String>), &'static str> {
    let mut included = BTreeSet::new();
    let mut excluded = BTreeSet::new();
    if value.is_empty() {
        return Ok((vec![], vec![]));
    }
    for item in value.split(delimiter) {
        let (negative, domain) = item.strip_prefix('~').map_or((false, item), |v| (true, v));
        if !host_valid(domain) {
            return Err("Unsupported domain scope");
        }
        if negative {
            excluded.insert(domain.to_ascii_lowercase());
        } else {
            included.insert(domain.to_ascii_lowercase());
        }
    }
    if included.len() > 200 || excluded.len() > 200 {
        return Err("Domain scope exceeds 200 entries");
    }
    Ok((
        included.into_iter().collect(),
        excluded.into_iter().collect(),
    ))
}

fn pattern_valid(pattern: &str) -> bool {
    if pattern.is_empty()
        || pattern.len() > 2048
        || pattern.starts_with("||*")
        || (pattern.starts_with('/') && pattern.ends_with('/') && pattern.len() > 1)
        || !pattern
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"\\$#".contains(&b))
    {
        return false;
    }
    let core = pattern
        .strip_prefix("||")
        .or_else(|| pattern.strip_prefix('|'))
        .unwrap_or(pattern);
    let core = core.strip_suffix('|').unwrap_or(core);
    !core.is_empty() && !core.contains('|')
}

fn anchored_host(pattern: &str) -> Option<String> {
    let rest = pattern.strip_prefix("||")?;
    let end = rest.find(['^', '/'])?;
    let host = &rest[..end];
    host_valid(host).then(|| host.to_ascii_lowercase())
}

fn resource(value: &str) -> Option<&'static str> {
    match value {
        "subdocument" => Some("sub_frame"),
        "script" => Some("script"),
        "image" => Some("image"),
        "stylesheet" => Some("stylesheet"),
        "object" => Some("object"),
        "xmlhttprequest" | "xhr" => Some("xmlhttprequest"),
        "ping" => Some("ping"),
        "media" => Some("media"),
        "font" => Some("font"),
        "websocket" => Some("websocket"),
        "other" => Some("other"),
        _ => None,
    }
}

fn network(raw: &str) -> Result<Vec<Value>, &'static str> {
    let exception = raw.starts_with("@@");
    let body = raw.strip_prefix("@@").unwrap_or(raw);
    let (pattern, options) = body.split_once('$').unwrap_or((body, ""));
    if !pattern_valid(pattern) {
        return Err("Unsupported network pattern or regular expression");
    }
    let mut condition = Map::new();
    condition.insert("urlFilter".into(), json!(pattern));
    let mut positive = BTreeSet::new();
    let mut negative = BTreeSet::new();
    let mut document = false;
    let mut seen = BTreeSet::new();
    for option in options.split(',').filter(|s| !s.is_empty()) {
        let key = option.split('=').next().unwrap_or(option);
        if !seen.insert(key) {
            return Err("Duplicate network modifier");
        }
        match option {
            "third-party" | "3p" | "~first-party" | "~1p" => {
                if condition
                    .insert("domainType".into(), json!("thirdParty"))
                    .is_some()
                {
                    return Err("Conflicting party modifiers");
                }
            }
            "~third-party" | "~3p" | "first-party" | "1p" => {
                if condition
                    .insert("domainType".into(), json!("firstParty"))
                    .is_some()
                {
                    return Err("Conflicting party modifiers");
                }
            }
            "match-case" => {
                condition.insert("isUrlFilterCaseSensitive".into(), json!(true));
            }
            "document" if exception => {
                document = true;
            }
            _ if option.starts_with("domain=") => {
                let value = &option[7..];
                if value.is_empty() {
                    return Err("Empty domain restriction");
                }
                let (yes, no) = scopes(value, '|')?;
                if !yes.is_empty() {
                    condition.insert("initiatorDomains".into(), json!(yes));
                }
                if !no.is_empty() {
                    condition.insert("excludedInitiatorDomains".into(), json!(no));
                }
            }
            _ => {
                let inverse = option.starts_with('~');
                let name = option.trim_start_matches('~');
                let Some(kind) = resource(name) else {
                    return Err("Unsupported network modifier");
                };
                if inverse {
                    negative.insert(kind);
                } else {
                    positive.insert(kind);
                }
            }
        }
    }
    let types: Vec<_> = if positive.is_empty() {
        RESOURCE_TYPES
            .iter()
            .copied()
            .filter(|k| !negative.contains(k))
            .collect()
    } else {
        positive
            .iter()
            .copied()
            .filter(|k| !negative.contains(k))
            .collect()
    };
    if types.is_empty() {
        return Err("Network resource restrictions match no supported type");
    }
    let mut result = vec![];
    if document {
        let mut main = condition.clone();
        main.insert("resourceTypes".into(), json!(["main_frame", "sub_frame"]));
        result.push(json!({"priority":2,"action":{"type":"allowAllRequests"},"condition":main}));
    }
    if !document || !positive.is_empty() || !negative.is_empty() {
        condition.insert("resourceTypes".into(), json!(types));
        result.push(json!({"priority":if exception {2} else {1},"action":{"type":if exception {"allow"} else {"block"}},"condition":condition}));
    }
    Ok(result)
}

#[derive(Default)]
struct Diagnostics {
    samples: Vec<Value>,
    reasons: BTreeMap<String, usize>,
    total: usize,
    sampled: BTreeMap<String, usize>,
}
impl Diagnostics {
    fn add(&mut self, source: &str, line: usize, raw: &str, reason: &str) {
        self.total += 1;
        *self.reasons.entry(reason.into()).or_default() += 1;
        let count = self.sampled.entry(reason.into()).or_default();
        if *count >= 5 || self.samples.len() >= MAX_DIAGNOSTICS {
            return;
        }
        *count += 1;
        let mut end = raw.len().min(2048);
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        self.samples.push(json!({"source":source,"line":line,"raw":&raw[..end],"target":"UNSUPPORTED","message":reason}));
    }
}

#[derive(Clone)]
struct Hide {
    domains: Vec<String>,
    excluded: Vec<String>,
    selector: String,
    source: String,
    raw: String,
    line: usize,
}

/// Pack only full hostname anchors. Paths, wildcards and differing conditions
/// remain separate, so compacting cannot broaden their match semantics.
fn pack_network(rules: BTreeMap<String, Value>) -> Vec<Value> {
    let mut output = BTreeMap::new();
    let mut groups: BTreeMap<String, (Value, BTreeSet<String>)> = BTreeMap::new();
    for (_, mut rule) in rules {
        let pattern = rule["condition"]["urlFilter"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        let host = pattern
            .strip_prefix("||")
            .and_then(|s| s.strip_suffix('^'))
            .filter(|h| host_valid(h));
        // Case-sensitive hostname filters aren't equivalent to normalized host matching.
        if let Some(host) = host.filter(|_| rule["condition"]["isUrlFilterCaseSensitive"] != true) {
            rule["condition"]
                .as_object_mut()
                .unwrap()
                .remove("urlFilter");
            let key = rule.to_string();
            groups
                .entry(key)
                .or_insert_with(|| (rule, BTreeSet::new()))
                .1
                .insert(host.to_ascii_lowercase());
        } else {
            output.insert(rule.to_string(), rule);
        }
    }
    for (_, (template, hosts)) in groups {
        let hosts: Vec<_> = hosts.into_iter().collect();
        for chunk in hosts.chunks(1000) {
            let mut rule = template.clone();
            rule["condition"]["requestDomains"] = json!(chunk);
            output.insert(rule.to_string(), rule);
        }
    }
    output.into_values().collect()
}

pub fn compile_subscriptions(
    sources: &[(&str, &str)],
    network_budget: usize,
) -> Result<Value, String> {
    if network_budget > 29_800 {
        return Err("Network budget exceeds 29800".into());
    }
    let mut ids = BTreeSet::new();
    for (id, text) in sources {
        if !["easylist", "easyprivacy"].contains(id) || !ids.insert(*id) {
            return Err("Unknown or duplicate source ID".into());
        }
        if text.len() > MAX_BYTES || text.lines().count() > MAX_LINES {
            return Err("Subscription input exceeds size or line limit".into());
        }
    }
    if sources.is_empty() {
        return Err("Select at least one source".into());
    }
    let mut diagnostic = Diagnostics::default();
    let mut network_rules = BTreeMap::new();
    let mut hides = vec![];
    let mut exception_domains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut suppressed_selectors = BTreeSet::new();
    let mut generic_excluded = BTreeSet::new();
    let mut all_excluded = BTreeSet::new();
    let mut suppress_generic = false;
    let mut suppress_cosmetic = false;
    let mut suppress_network_blocks = false;
    let mut badfilters = BTreeSet::new();
    let mut list_stats = vec![];
    let mut ignored_total = 0;
    let mut unsupported_total = 0;
    for (source, text) in sources {
        let mut accepted_network = 0;
        let mut accepted_cosmetic = 0;
        let mut unsupported = 0;
        let mut ignored = 0;
        for (index, original) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let raw = original.trim();
            let line = index + 1;
            if raw.is_empty() || raw.starts_with('!') || raw.starts_with("[Adblock") {
                ignored += 1;
                continue;
            }
            if raw.len() > 2048 && !raw.starts_with("@@") && !raw.contains("#@#") {
                unsupported += 1;
                diagnostic.add(source, line, raw, "Rule exceeds 2048 bytes");
                continue;
            }
            if let Some((scope, selector)) = raw.split_once("#@#") {
                // Canonicalize supported child-check whitespace before matching.
                if !valid_selector(selector) {
                    unsupported += 1;
                    diagnostic.add(source, line, raw, "Unsupported cosmetic exception selector");
                    continue;
                }
                let selector = canonical_selector(selector);
                match scopes(scope, ',') {
                    Ok((yes, no)) if !yes.is_empty() && no.is_empty() => {
                        exception_domains
                            .entry(selector.into())
                            .or_default()
                            .extend(yes);
                    }
                    _ => {
                        suppressed_selectors.insert(selector.to_owned());
                    }
                }
                ignored += 1;
                continue;
            }
            if let Some((scope, selector)) = raw.split_once("##") {
                let parsed = scopes(scope, ',');
                if valid_selector(selector) && parsed.is_ok() {
                    let (domains, excluded) = parsed.unwrap();
                    accepted_cosmetic += 1;
                    hides.push(Hide {
                        domains,
                        excluded,
                        selector: canonical_selector(selector),
                        source: (*source).into(),
                        raw: raw.into(),
                        line,
                    });
                } else {
                    unsupported += 1;
                    diagnostic.add(source, line, raw, "Unsupported cosmetic selector or scope");
                }
                continue;
            }
            if raw.contains('#') {
                unsupported += 1;
                diagnostic.add(
                    source,
                    line,
                    raw,
                    "Scriptlets and extended cosmetic syntax are unsupported",
                );
                continue;
            }
            let body = raw.strip_prefix("@@").unwrap_or(raw);
            let (pattern, opts) = body.split_once('$').unwrap_or((body, ""));
            let options: Vec<_> = opts.split(',').collect();
            if options.contains(&"badfilter") {
                let stripped = format!(
                    "{}${}",
                    raw.split('$').next().unwrap_or(""),
                    options
                        .iter()
                        .copied()
                        .filter(|s| *s != "badfilter")
                        .collect::<Vec<_>>()
                        .join(",")
                );
                match network(stripped.trim_end_matches('$')) {
                    Ok(rules) => {
                        for rule in rules {
                            badfilters.insert(rule.to_string());
                        }
                        ignored += 1;
                    }
                    Err(_) => {
                        unsupported += 1;
                        diagnostic.add(source, line, raw, "Unsupported badfilter target");
                    }
                }
                continue;
            }
            let mut translated = raw.to_owned();
            if raw.starts_with("@@")
                && (options.contains(&"generichide")
                    || options.contains(&"elemhide")
                    || options.contains(&"document"))
            {
                let all = options.contains(&"elemhide") || options.contains(&"document");
                // Broader hostname exclusion for path-specific cosmetic exemptions
                // loses cosmetic coverage but cannot over-hide an exempt page.
                let domains = if let Some(domain_option) =
                    options.iter().find_map(|s| s.strip_prefix("domain="))
                {
                    scopes(domain_option, '|')
                        .ok()
                        .filter(|(yes, no)| !yes.is_empty() && no.is_empty())
                        .map(|(yes, _)| yes)
                } else {
                    anchored_host(pattern).map(|host| vec![host])
                };
                if let Some(domains) = domains {
                    if all {
                        all_excluded.extend(domains);
                    } else {
                        generic_excluded.extend(domains);
                    }
                } else if all {
                    suppress_cosmetic = true;
                } else {
                    suppress_generic = true;
                }
                if options.contains(&"generichide") || options.contains(&"elemhide") {
                    diagnostic.add(source,line,raw,"Cosmetic exemption applied conservatively; some hiding coverage may be omitted");
                    let remaining: Vec<_> = options
                        .iter()
                        .copied()
                        .filter(|s| !["generichide", "elemhide"].contains(s))
                        .collect();
                    let has_network_action = remaining.iter().any(|s| {
                        !s.starts_with("domain=")
                            && ![
                                "third-party",
                                "~third-party",
                                "first-party",
                                "~first-party",
                                "match-case",
                                "",
                            ]
                            .contains(s)
                    });
                    if !has_network_action {
                        ignored += 1;
                        continue;
                    }
                    translated = format!("@@{pattern}${}", remaining.join(","));
                }
            }
            // A document exemption protects the target page, even when an
            // unsupported modifier forces a broader network allowance below.
            // domain= may name its parent/initiator rather than that target.
            if raw.starts_with("@@") && options.contains(&"document") {
                if let Some(host) = anchored_host(pattern) {
                    all_excluded.insert(host);
                } else {
                    suppress_cosmetic = true;
                }
            }
            match network(&translated) {
                Ok(rules) => {
                    accepted_network += 1;
                    for rule in rules {
                        network_rules.insert(rule.to_string(), rule);
                    }
                }
                Err(reason) => {
                    unsupported += 1;
                    diagnostic.add(source, line, raw, reason);
                    if raw.starts_with("@@") {
                        // Preserve the URL boundary while relaxing unsupported context.
                        // If even that boundary is unknown, emit no subscription blocks.
                        if pattern_valid(pattern) {
                            let guard = json!({"priority":2,"action":{"type":"allow"},"condition":{"urlFilter":pattern,"resourceTypes":RESOURCE_TYPES}});
                            network_rules.insert(guard.to_string(), guard);
                            if options.contains(&"document") || options.contains(&"genericblock") {
                                let frame_guard = json!({"priority":2,"action":{"type":"allowAllRequests"},"condition":{"urlFilter":pattern,"resourceTypes":["main_frame","sub_frame"]}});
                                network_rules.insert(frame_guard.to_string(), frame_guard);
                            }
                            diagnostic.add(
                                source,
                                line,
                                raw,
                                "Unsupported exception protected by a broader allow guard",
                            );
                        } else {
                            suppress_network_blocks = true;
                            diagnostic.add(
                                source,
                                line,
                                raw,
                                "Unrepresentable exception suppresses subscription network blocks",
                            );
                        }
                    }
                }
            }
        }
        ignored_total += ignored;
        unsupported_total += unsupported;
        list_stats.push(json!({"id":source,"network":accepted_network,"cosmetic":accepted_cosmetic,"unsupported":unsupported,"ignored":ignored}));
    }
    for key in badfilters {
        network_rules.remove(&key);
    }
    let mut rules = pack_network(network_rules);
    let mut safety_suppressed = 0;
    if suppress_network_blocks {
        let before = rules.len();
        rules.retain(|r| r["action"]["type"] != "block");
        safety_suppressed += before - rules.len();
    }
    let network_supported = rules.len();
    rules.sort_by_cached_key(|r| (r["action"]["type"] == "block", r.to_string()));
    let exception_count = rules
        .iter()
        .filter(|r| r["action"]["type"] != "block")
        .count();
    if exception_count > network_budget {
        return Err(
            "Supported exceptions exceed the browser rule budget; previous rules must be retained"
                .into(),
        );
    }
    let network_dropped = rules.len().saturating_sub(network_budget);
    rules.truncate(network_budget);
    for (index, rule) in rules.iter_mut().enumerate() {
        rule["id"] = json!(index + 1);
    }
    let cosmetic_supported = hides.len();
    let mut cosmetics = BTreeMap::new();
    for hide in hides {
        let mut excluded: BTreeSet<_> = hide.excluded.into_iter().collect();
        excluded.extend(all_excluded.iter().cloned());
        if hide.domains.is_empty() {
            excluded.extend(generic_excluded.iter().cloned());
        }
        if let Some(domains) = exception_domains.get(&hide.selector) {
            excluded.extend(domains.iter().cloned());
        }
        if suppress_cosmetic
            || (suppress_generic && hide.domains.is_empty())
            || suppressed_selectors.contains(&hide.selector)
            || excluded.len() > 200
        {
            safety_suppressed += 1;
            diagnostic.add(
                &hide.source,
                hide.line,
                &hide.raw,
                "Cosmetic rule omitted to honor an exemption safely",
            );
            continue;
        }
        let excluded: Vec<_> = excluded.into_iter().collect();
        let key = json!([hide.domains, excluded, hide.selector]).to_string();
        cosmetics.entry(key).or_insert_with(|| json!({"domains":hide.domains,"excludedDomains":excluded,"selector":hide.selector,"raw":hide.raw,"source":hide.source}));
    }
    let cosmetic_dropped = cosmetics.len().saturating_sub(MAX_COSMETIC);
    let cosmetics: Vec<_> = cosmetics.into_values().take(MAX_COSMETIC).collect();
    let reasons: Vec<_> = diagnostic
        .reasons
        .into_iter()
        .map(|(reason, count)| json!({"reason":reason,"count":count}))
        .collect();
    let samples_count = diagnostic.samples.len();
    Ok(json!({
        "stats":{"network":rules.len(),"cosmetic":cosmetics.len(),"unsupported":unsupported_total,"ignored":ignored_total},
        "coverage":{"networkSupported":network_supported,"networkDropped":network_dropped,"cosmeticSupported":cosmetic_supported,"cosmeticDropped":cosmetic_dropped,"exceptionSafetySuppressed":safety_suppressed,"diagnosticsTotal":diagnostic.total,"diagnosticsTruncated":diagnostic.total>samples_count},
        "networkRules":rules,"cosmeticRules":cosmetics,"diagnostics":diagnostic.samples,"listStats":list_stats,"unsupportedReasons":reasons
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compile(text: &str) -> Value {
        compile_subscriptions(&[("easylist", text)], 27800).unwrap()
    }
    #[test]
    fn compacts_hostnames_without_dropping_domains() {
        let input = (0..2501)
            .map(|n| format!("||ad{n}.test^\n"))
            .collect::<String>();
        let v = compile(&input);
        assert_eq!(v["stats"]["network"], 3);
        assert_eq!(
            v["networkRules"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["condition"]["requestDomains"].as_array().unwrap().len())
                .sum::<usize>(),
            2501
        );
        assert_eq!(v["coverage"]["networkDropped"], 0);
    }
    #[test]
    fn context_resource_and_wildcard_rules_keep_their_conditions() {
        let v = compile(
            "||ads.test/path*$script,~third-party,domain=news.test|~safe.news.test,match-case",
        );
        let c = &v["networkRules"][0]["condition"];
        assert_eq!(c["urlFilter"], "||ads.test/path*");
        assert_eq!(c["resourceTypes"], json!(["script"]));
        assert_eq!(c["domainType"], "firstParty");
        assert_eq!(c["initiatorDomains"], json!(["news.test"]));
        assert_eq!(c["excludedInitiatorDomains"], json!(["safe.news.test"]));
        assert_eq!(c["isUrlFilterCaseSensitive"], true);
    }
    #[test]
    fn different_types_are_never_packed_together() {
        let v = compile("||ad.test^$image\n||script.test^$script\n@@||ad.test^$image");
        assert_eq!(v["stats"]["network"], 3);
    }
    #[test]
    fn page_exception_keeps_mainframe_and_subframe_semantics_separate() {
        let v = compile("@@||safe.test/path|$document,subdocument");
        let a = v["networkRules"].as_array().unwrap();
        assert_eq!(a.len(), 2);
        assert!(a.iter().any(|r| r["action"]["type"] == "allowAllRequests"
            && r["condition"]["resourceTypes"] == json!(["main_frame", "sub_frame"])));
        assert!(a.iter().any(|r| r["action"]["type"] == "allow"
            && r["condition"]["resourceTypes"] == json!(["sub_frame"])));
    }
    #[test]
    fn cosmetic_exceptions_work_across_lists() {
        let v = compile_subscriptions(
            &[
                ("easylist", "##.ad\nnews.test##.sponsor"),
                ("easyprivacy", "news.test#@#.ad\nnews.test#@#.sponsor"),
            ],
            100,
        )
        .unwrap();
        for r in v["cosmeticRules"].as_array().unwrap() {
            assert!(r["excludedDomains"]
                .as_array()
                .unwrap()
                .contains(&json!("news.test")));
        }
    }
    #[test]
    fn unrepresentable_cosmetic_exemption_fails_open() {
        let v = compile("##.ad\nnews.test##.sponsor\n@@://10.0.0.$generichide");
        assert_eq!(v["stats"]["cosmetic"], 1);
        assert_eq!(v["coverage"]["exceptionSafetySuppressed"], 1);
    }
    #[test]
    fn unsupported_network_exception_installs_allow_guard() {
        let v = compile("||ads.test^\n@@||ads.test/path$unknown");
        assert!(v["networkRules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"]["type"] == "allow"
                && r["condition"]["urlFilter"] == "||ads.test/path"));
    }
    #[test]
    fn unrepresentable_network_exception_suppresses_blocks() {
        let v = compile("||ads.test^\n@@/unknown.+/$script");
        assert_eq!(v["stats"]["network"], 0);
        assert_eq!(v["coverage"]["exceptionSafetySuppressed"], 1);
    }
    #[test]
    fn badfilter_cancels_matching_rule_in_other_list() {
        let v = compile_subscriptions(
            &[
                ("easylist", "||ads.test^$script"),
                ("easyprivacy", "||ads.test^$script,badfilter"),
            ],
            100,
        )
        .unwrap();
        assert_eq!(v["stats"]["network"], 0);
    }
    #[test]
    fn budget_never_drops_exception_to_keep_block() {
        let v = compile_subscriptions(&[("easylist", "||ads.test/path\n@@||ads.test/safe")], 1)
            .unwrap();
        assert_eq!(v["networkRules"][0]["action"]["type"], "allow");
        assert_eq!(v["coverage"]["networkDropped"], 1);
        assert!(compile_subscriptions(&[("easylist", "@@||ads.test/safe")], 0).is_err());
    }
    #[test]
    fn diagnostic_samples_and_large_lines_are_bounded() {
        let text = "||ads.test^$unknown\n".repeat(4000);
        let v = compile(&text);
        assert_eq!(v["stats"]["unsupported"], 4000);
        assert!(v["diagnostics"].as_array().unwrap().len() <= 200);
        assert_eq!(v["coverage"]["diagnosticsTruncated"], true);
        let v = compile(&format!("{}\n||good.test^", "é".repeat(2000)));
        assert_eq!(v["stats"]["network"], 1);
        assert!(v["diagnostics"][0]["raw"].as_str().unwrap().len() <= 2048);
    }
    #[test]
    fn mainframes_are_never_blocked_and_ids_are_unique_deterministic() {
        let v = compile("||a.test^\n||b.test/script\n@@||c.test^\n||a.test^\n");
        let other = compile("@@||c.test^\n||b.test/script\n||a.test^");
        assert_eq!(v["networkRules"], other["networkRules"]);
        let mut ids = BTreeSet::new();
        for rule in v["networkRules"].as_array().unwrap() {
            assert!(ids.insert(rule["id"].as_u64().unwrap()));
            if rule["action"]["type"] == "block" {
                assert!(!rule["condition"]["resourceTypes"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("main_frame")));
            }
        }
    }
    #[test]
    fn input_validation_is_bounded() {
        assert!(compile_subscriptions(&[("unknown", "a")], 100).is_err());
        assert!(compile_subscriptions(&[("easylist", "a"), ("easylist", "b")], 100).is_err());
        assert!(compile_subscriptions(&[("easylist", "a")], 30000).is_err());
        assert!(compile_subscriptions(&[], 100).is_err());
    }
    #[test]
    fn long_exception_retains_unrelated_blocks_and_hides() {
        let domains = (0..500)
            .map(|n| format!("page{n}.example"))
            .collect::<Vec<_>>()
            .join("|");
        let v = compile(&format!(
            "||ads.test^\nnews.test##.ad\n@@||tagmanager.test/gtm.js$domain={domains}"
        ));
        assert_eq!(v["stats"]["cosmetic"], 1);
        assert!(v["networkRules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"]["type"] == "block"));
        assert!(v["networkRules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"]["type"] == "allow"
                && r["condition"]["urlFilter"] == "||tagmanager.test/gtm.js"));
    }
    #[test]
    fn mixed_page_exemptions_preserve_descendant_network_and_cosmetic_guards() {
        for option in [
            "document,generichide",
            "document,unknown",
            "document,domain=parent.test,unknown",
            "genericblock",
        ] {
            let v = compile(&format!(
                "||ads.test^\nsafe.test##.ad\n@@||safe.test^${option}"
            ));
            assert!(
                v["networkRules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["action"]["type"] == "allowAllRequests"
                        && r["condition"]["resourceTypes"] == json!(["main_frame", "sub_frame"])),
                "{option}"
            );
            if option.starts_with("document") {
                assert!(v["cosmeticRules"][0]["excludedDomains"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("safe.test")));
            }
        }
        let v = compile("safe.test##.ad\n@@||safe.test$document,domain=parent.test,unknown");
        assert_eq!(v["stats"]["cosmetic"], 0);
    }
    #[test]
    fn boundaryless_cosmetic_exemption_is_never_narrowed() {
        assert_eq!(anchored_host("||safe.test"), None);
        let v = compile("##.ad\n@@||safe.test$generichide");
        assert_eq!(v["stats"]["cosmetic"], 0);
    }

    #[test]
    fn direct_child_cosmetics_preserve_domain_exceptions() {
        let v = compile("site.test##div:has(> .ad-label)\nsite.test##div:has(>.ad-label)\nsafe.site.test#@#div:has(>  .ad-label  )");
        assert_eq!(v["stats"]["cosmetic"], 1);
        assert_eq!(v["cosmeticRules"][0]["selector"], "div:has(>.ad-label)");
        assert_eq!(v["cosmeticRules"][0]["domains"], json!(["site.test"]));
        assert!(v["cosmeticRules"][0]["excludedDomains"]
            .as_array()
            .unwrap()
            .contains(&json!("safe.site.test")));
    }
}
