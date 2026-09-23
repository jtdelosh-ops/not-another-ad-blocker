use naab_companion::dns::blocklist::{DnsPolicy, RuleSource};

struct Fixture {
    name: &'static str,
    text: &'static str,
    blocks: usize,
    allows: usize,
    conservative_lines: usize,
    ignored: usize,
    unsupported: usize,
    suppressed: bool,
    probes: &'static [(&'static str, bool)],
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "unconditional hostname block",
        text: "||ads.example^",
        blocks: 1,
        allows: 0,
        conservative_lines: 0,
        ignored: 0,
        unsupported: 0,
        suppressed: false,
        probes: &[
            ("ads.example", true),
            ("sub.ads.example", true),
            ("notads.example", false),
        ],
    },
    Fixture {
        name: "hostname exception",
        text: "||ads.example^\n@@||ads.example^",
        blocks: 1,
        allows: 1,
        conservative_lines: 0,
        ignored: 0,
        unsupported: 0,
        suppressed: false,
        probes: &[("ads.example", false), ("sub.ads.example", false)],
    },
    Fixture {
        name: "contextual and path rules",
        text: "||path.example/ads.js\n||third.example^$third-party\n||image.example^$image\n/ads/",
        blocks: 0,
        allows: 0,
        conservative_lines: 0,
        ignored: 0,
        unsupported: 4,
        suppressed: false,
        probes: &[
            ("path.example", false),
            ("third.example", false),
            ("image.example", false),
        ],
    },
    Fixture {
        name: "comments and cosmetic rules",
        text: "! comment\nsite.example##.ad\n@@*$generichide\n",
        blocks: 0,
        allows: 0,
        conservative_lines: 0,
        ignored: 3,
        unsupported: 0,
        suppressed: false,
        probes: &[("site.example", false)],
    },
    Fixture {
        name: "conservative hostname exception",
        text: "||ads.example^\n||other.example^\n@@||ads.example/allowed.js",
        blocks: 2,
        allows: 0,
        conservative_lines: 1,
        ignored: 0,
        unsupported: 1,
        suppressed: false,
        probes: &[
            ("ads.example", false),
            ("sub.ads.example", false),
            ("other.example", true),
        ],
    },
    Fixture {
        name: "page-wide exception safety suppression",
        text: "||ads.example^\n@@||site.example^$document",
        blocks: 1,
        allows: 0,
        conservative_lines: 0,
        ignored: 0,
        unsupported: 1,
        suppressed: true,
        probes: &[("ads.example", false)],
    },
    Fixture {
        name: "preprocessing directive safety suppression",
        text: "!#if false\n||ads.example^\n!#endif",
        blocks: 1,
        allows: 0,
        conservative_lines: 0,
        ignored: 0,
        unsupported: 2,
        suppressed: true,
        probes: &[("ads.example", false)],
    },
];

#[test]
fn representative_rules_match_dns_compatibility_contract() {
    for fixture in FIXTURES {
        let policy = DnsPolicy::compile(
            &[RuleSource {
                name: fixture.name.to_owned(),
                text: fixture.text.to_owned(),
            }],
            &[],
            &[],
        )
        .unwrap();
        let report = policy.report();
        let source = &report.sources[0];
        assert_eq!(source.blocks, fixture.blocks, "{}", fixture.name);
        assert_eq!(source.allows, fixture.allows, "{}", fixture.name);
        assert_eq!(
            source.conservative_allows, fixture.conservative_lines,
            "{} ({source:?})",
            fixture.name
        );
        assert_eq!(source.ignored, fixture.ignored, "{}", fixture.name);
        assert_eq!(source.unsupported, fixture.unsupported, "{}", fixture.name);
        assert_eq!(
            report.list_blocks_suppressed, fixture.suppressed,
            "{}",
            fixture.name
        );
        assert_eq!(
            report.effective_list_block_rules,
            if fixture.suppressed {
                0
            } else {
                fixture.blocks
            },
            "{}",
            fixture.name
        );
        for &(hostname, blocked) in fixture.probes {
            assert_eq!(
                policy.decide(hostname).blocked,
                blocked,
                "{}: {hostname}",
                fixture.name
            );
        }
    }
}

#[test]
fn user_overrides_remain_independent_of_list_compatibility() {
    let policy = DnsPolicy::compile(
        &[RuleSource {
            name: "override fixture".into(),
            text: "||ads.example^\n||allowed.example^\n@@||safe.example/allowed.js".into(),
        }],
        &["allowed.example".into()],
        &["forced.example".into()],
    )
    .unwrap();

    assert!(policy.decide("ads.example").blocked);
    assert!(!policy.decide("allowed.example").blocked);
    assert!(policy.decide("forced.example").blocked);
    assert!(!policy.report().list_blocks_suppressed);
}
