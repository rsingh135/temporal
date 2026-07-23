//! Installed-app capability catalog: the knowledge that lets Summon propose
//! *launching* an app the user hasn't opened yet ("work on the app UI" → open
//! Figma even though no snapshot ever held it).
//!
//! The scanner (macos-ffi) lists installed `.app` bundles; this module supplies
//! the capability vocabulary, a seed `bundle-id → capabilities` table, the text
//! each entry embeds as, and the pure rule that picks which apps to launch for
//! an intent. Everything here is deterministic so it unit-tests without models
//! or a filesystem.

use std::collections::HashSet;

/// Fixed capability vocabulary. The LLM intent planner is constrained to these
/// labels so its output can be matched against the seed table symbolically —
/// keep the planner prompt and this list in sync.
pub const CAPABILITIES: &[&str] = &[
    "design",
    "editor",
    "build",
    "terminal",
    "browser",
    "vcs",
    "database",
    "api-client",
    "container",
    "planning",
    "notes",
    "comms",
    "email",
    "calendar",
    "music",
    "video",
    "reading",
    "spreadsheet",
    "ai-chat",
];

/// True if `s` is a recognized capability label. Used to filter LLM output down
/// to the known vocabulary before symbolic matching.
pub fn is_capability(s: &str) -> bool {
    CAPABILITIES.contains(&s)
}

/// Seed capabilities for well-known apps, keyed by bundle id. Unknown apps get
/// an empty slice and match on name similarity alone. The list is intentionally
/// hand-curated (precision over coverage); usage feedback refines it later (see
/// the plan's affinity loop). Bundle ids verified against shipping apps.
pub fn seed_capabilities(bundle_id: &str) -> &'static [&'static str] {
    match bundle_id {
        // Design
        "com.figma.Desktop" => &["design"],
        "com.bohemiancoding.sketch3" => &["design"],
        "com.adobe.Photoshop" | "com.adobe.illustrator" | "com.adobe.xd" => &["design"],
        "com.seriflabs.affinitydesigner2" | "com.seriflabs.affinityphoto2" => &["design"],

        // Editors / IDEs (also build)
        "com.microsoft.VSCode" => &["editor", "build"],
        "com.todesktop.230313mzl4w4u92" => &["editor", "build"], // Cursor
        "com.apple.dt.Xcode" => &["editor", "build"],
        "com.sublimetext.4" | "com.sublimetext.3" => &["editor"],
        "dev.zed.Zed" => &["editor", "build"],
        "com.jetbrains.intellij" | "com.jetbrains.pycharm" | "com.jetbrains.WebStorm"
        | "com.jetbrains.rustrover" | "com.jetbrains.goland" | "com.jetbrains.CLion" => {
            &["editor", "build"]
        }
        "com.panic.Nova" => &["editor", "build"],

        // Terminals
        "com.apple.Terminal" => &["terminal"],
        "com.googlecode.iterm2" => &["terminal"],
        "dev.warp.Warp-Stable" => &["terminal"],
        "net.kovidgoyal.kitty" | "com.github.wez.wezterm" | "io.alacritty" => &["terminal"],

        // Browsers
        "com.google.Chrome" => &["browser"],
        "com.apple.Safari" => &["browser"],
        "org.mozilla.firefox" => &["browser"],
        "com.microsoft.edgemac" => &["browser"],
        "company.thebrowser.Browser" => &["browser"], // Arc
        "com.brave.Browser" => &["browser"],

        // Version control
        "com.github.GitHubClient" | "com.fournova.Tower3" | "com.sublimemerge"
        | "com.torusknot.SourceTreeNotMAS" | "co.gitup.mac" => &["vcs"],

        // Databases
        "com.tableplus.TablePlus" | "com.sequel-ace.sequel-ace" | "com.eggerapps.Postico2"
        | "com.mongodb.compass" | "com.electron.dbeaver" => &["database"],

        // API clients
        "com.postmanlabs.mac" | "com.insomnia.app" | "com.usebruno.app"
        | "paw.cloud.RESTClient" => &["api-client"],

        // Containers / infra
        "com.docker.docker" => &["container"],
        "com.orbstack.OrbStack" => &["container"],

        // Planning / project management
        "com.linear" | "com.electron.asana" | "com.atlassian.trello"
        | "com.microsoft.to-do-mac" => &["planning"],

        // Notes / knowledge
        "notion.id" => &["notes", "planning"],
        "md.obsidian" => &["notes"],
        "com.apple.Notes" => &["notes"],
        "com.agiletortoise.Drafts-OSX" | "com.shinyfrog.bear" => &["notes"],

        // Communication
        "com.tinyspeck.slackmacgap" => &["comms"],
        "com.hnc.Discord" => &["comms"],
        "us.zoom.xos" => &["comms", "video"],
        "com.microsoft.teams2" => &["comms", "video"],
        "com.apple.iChat" => &["comms"], // Messages

        // Email / calendar
        "com.apple.mail" => &["email"],
        "com.readdle.smartemail-Mac" => &["email"],       // Spark
        "com.superhuman.electron" => &["email"],
        "com.apple.iCal" => &["calendar"],
        "com.flexibits.fantastical2.mac" => &["calendar"],

        // Media
        "com.spotify.client" => &["music"],
        "com.apple.Music" => &["music"],

        // Reading / docs
        "com.readdle.PDFExpert-Mac" | "com.apple.Preview" => &["reading"],

        // Office / spreadsheets
        "com.microsoft.Excel" | "com.apple.iWork.Numbers" => &["spreadsheet"],
        "com.microsoft.Word" | "com.apple.iWork.Pages" => &["notes"],

        // AI chat
        "com.openai.chat" => &["ai-chat"],
        "com.anthropic.claudefordesktop" => &["ai-chat"],

        _ => &[],
    }
}

/// Text embedded to represent a catalog entry. Capabilities are genuine
/// semantic signal — they describe what the app is *for* — so they belong in
/// the vector, unlike the generic item-kind prefixes that collapse item
/// clustering (see `grouping`). Name first, capabilities after.
pub fn catalog_embed_text(app_name: &str, capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        app_name.to_string()
    } else {
        format!("{app_name} {}", capabilities.join(" "))
    }
}

/// One installed app scored against an intent, ready for selection.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogCandidate {
    pub bundle_id: String,
    pub app_name: String,
    pub capabilities: Vec<String>,
    /// Cosine similarity of the entry's embedding to the intent, in [0, 1].
    pub score: f64,
}

/// A catalog entry qualifies on embedding alone only above this bar. Launching
/// an app the user didn't open is speculative, and a wrong launch is more
/// annoying than a missing one — so pure-embedding picks are held to a high
/// threshold. A capability match (below) is a stronger, symbolic signal and
/// bypasses it.
pub const CATALOG_EMBED_ONLY_MIN: f64 = 0.62;

/// Even a capability-matched app needs *some* embedding relevance, so an intent
/// that merely implies "editor" doesn't drag in every installed editor.
pub const CATALOG_CAP_MATCH_MIN: f64 = 0.30;

/// Chooses which installed apps to speculatively launch for an intent. An app
/// qualifies if it shares a capability with the inferred set (and clears the
/// low relevance floor), or if its embedding is a strong match on its own.
/// Apps already present (running / in the assembled set) are excluded — Summon
/// never proposes launching something that's already there. Ranked by
/// (capability overlap, score) and capped.
pub fn select_catalog_picks(
    candidates: &[CatalogCandidate],
    inferred_caps: &[String],
    already_present: &HashSet<String>,
    max_picks: usize,
) -> Vec<CatalogCandidate> {
    let inferred: HashSet<&str> = inferred_caps.iter().map(String::as_str).collect();
    let mut scored: Vec<(usize, CatalogCandidate)> = candidates
        .iter()
        .filter(|c| !already_present.contains(&c.bundle_id))
        .filter_map(|c| {
            let overlap =
                c.capabilities.iter().filter(|cap| inferred.contains(cap.as_str())).count();
            // Embed-only qualification requires known capabilities: an app we
            // can't reason about (no seed capabilities) must never be launched
            // on a fuzzy name match alone — that's how "temporal" summons
            // "Time Machine". Symbolic capability overlap is the strong signal.
            let qualifies = (overlap > 0 && c.score >= CATALOG_CAP_MATCH_MIN)
                || (!c.capabilities.is_empty() && c.score >= CATALOG_EMBED_ONLY_MIN);
            qualifies.then(|| (overlap, c.clone()))
        })
        .collect();
    // Highest capability overlap first, then strongest embedding; stable-ish on
    // bundle id so ties are deterministic.
    scored.sort_by(|(oa, a), (ob, b)| {
        ob.cmp(oa)
            .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.bundle_id.cmp(&b.bundle_id))
    });
    scored.into_iter().take(max_picks).map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(bundle: &str, name: &str, caps: &[&str], score: f64) -> CatalogCandidate {
        CatalogCandidate {
            bundle_id: bundle.into(),
            app_name: name.into(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            score,
        }
    }

    #[test]
    fn seed_and_vocabulary_stay_in_sync() {
        // Every capability the seed table emits must be a known vocabulary word,
        // or symbolic matching against LLM output silently misses.
        for bundle in ["com.figma.Desktop", "com.microsoft.VSCode", "us.zoom.xos"] {
            for cap in seed_capabilities(bundle) {
                assert!(is_capability(cap), "seed cap {cap:?} not in vocabulary");
            }
        }
    }

    #[test]
    fn embed_text_omits_caps_when_empty_and_never_prefixes_kind() {
        assert_eq!(catalog_embed_text("Figma", &["design".into()]), "Figma design");
        assert_eq!(catalog_embed_text("Weird App", &[]), "Weird App");
    }

    #[test]
    fn capability_match_beats_pure_embedding_and_respects_floor() {
        let cands = vec![
            cand("com.figma.Desktop", "Figma", &["design"], 0.41), // cap match, above floor
            cand("com.apple.Music", "Music", &["music"], 0.50),    // irrelevant cap, below embed bar
            cand("com.jetbrains.WebStorm", "WebStorm", &["editor"], 0.71), // known app, embed-only
            cand("com.weak.design", "Weak", &["design"], 0.12),    // cap match but below floor
        ];
        let picks = select_catalog_picks(&cands, &["design".into()], &HashSet::new(), 3);
        let ids: Vec<&str> = picks.iter().map(|c| c.bundle_id.as_str()).collect();
        // Figma first (capability overlap), then the strong embed-only match on a
        // known app. Music has no matched capability and doesn't clear the
        // embed-only bar; Weak's design cap matches but is below the floor.
        assert_eq!(ids, vec!["com.figma.Desktop", "com.jetbrains.WebStorm"]);
    }

    #[test]
    fn unknown_app_never_launches_on_name_match_alone() {
        // No seed capabilities + a strong (even spurious) embedding match must
        // not qualify — this is the "temporal" → "Time Machine" guard.
        let cands = vec![cand("com.apple.TimeMachine", "Time Machine", &[], 0.95)];
        assert!(select_catalog_picks(&cands, &[], &HashSet::new(), 3).is_empty());
    }

    #[test]
    fn excludes_already_present_apps() {
        let cands = vec![cand("com.figma.Desktop", "Figma", &["design"], 0.55)];
        let present: HashSet<String> = ["com.figma.Desktop".to_string()].into_iter().collect();
        assert!(select_catalog_picks(&cands, &["design".into()], &present, 3).is_empty());
    }

    #[test]
    fn caps_the_number_of_picks() {
        let cands: Vec<CatalogCandidate> = (0..10)
            .map(|i| cand(&format!("com.app.{i}"), "App", &["editor"], 0.5))
            .collect();
        assert_eq!(select_catalog_picks(&cands, &["editor".into()], &HashSet::new(), 3).len(), 3);
    }
}
