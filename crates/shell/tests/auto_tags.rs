//! Automatic tags (WP-9.11; spec: browser-tags R49 to R58), driven headless
//! through `Shell::run`, as agents drive them: plug-ins of each format get
//! the Function tags their categories map to, samples get theirs from their
//! sound, a person's own tags stay apart from them, a tag a person takes
//! off stays off, and Include Auto Tags switches them in filters and `#`
//! search. Analysis is paced by hand, so no test waits on the clock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_shell::Shell;
use bl_shell::browser_commands::{
    self as ids, BrowserState, FilterViewState, ItemTag, TagSource, TagsState,
};
use bl_shell::plugin_commands::{self as plugin_ids, PluginScan};
use bl_shell::settings::Preferences;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-auto-tags-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixtures(folder: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(folder)
}

/// A shell on the null device, with its configuration folder (and so a real
/// database file), loading the preferences kept there, and analysis paced by
/// hand.
fn shell_on(config: &Path) -> Shell {
    let mut prefs = Preferences::load(config).unwrap_or_default();
    // The browser lists VST3 plug-ins and, on macOS, Audio Units while their
    // Plug-In Sources switches are on.
    prefs.vst3_system_folders = true;
    prefs.use_audio_units_v2 = true;
    let mut shell = Shell::new(
        prefs,
        Some(config.to_path_buf()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell.pace_analysis_by_hand();
    shell
}

fn run<T: DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

/// What `browser.tags` says an item carries.
fn carried(shell: &mut Shell, item: &str) -> Vec<ItemTag> {
    let tags: TagsState = run(shell, ids::TAGS, json!({ "items": [item] }));
    tags.items.into_iter().next().unwrap().tags
}

/// The full names of the automatic tags an item carries.
fn auto(shell: &mut Shell, item: &str) -> Vec<String> {
    carried(shell, item)
        .into_iter()
        .filter(|tag| tag.source == TagSource::Auto)
        .map(|tag| tag.tag)
        .collect()
}

fn ready() -> bool {
    bl_plugin_host::scan::scan_program().is_some()
}

/// Whether the scanner's child can load lilv.
fn lilv() -> bool {
    cfg!(target_os = "linux") || std::env::var_os("BL_TEST_LILV").is_some()
}

fn scan(shell: &mut Shell, path: &Path) -> PluginScan {
    run(shell, plugin_ids::SCAN, json!({ "paths": [text(path)] }))
}

/// The key a plug-in's tags are kept under.
fn plugin_key(plugin: &bl_shell::plugin_commands::ScannedItem) -> String {
    bl_plugin_host::scan::hide_key(plugin.format, &plugin.plugin_id)
}

// Spec: browser-tags R49, R50, R57. A plug-in of each format gets the
// Function tags its categories map to, marked automatic.
#[test]
fn test_plug_ins_of_each_format_get_the_function_tags_their_categories_map_to() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let scratch = Scratch::new("formats");
    let mut shell = shell_on(&scratch.path("config"));
    let scans = [
        scan(&mut shell, bl_test_clap::bundle()),
        scan(&mut shell, bl_test_vst3::bundle()),
    ];
    let mut keys: Vec<String> = scans
        .iter()
        .flat_map(|scan| scan.plugins.iter().map(plugin_key))
        .collect();
    // The browser lists LV2 plug-ins on Linux only; the other formats' tags are
    // tried here, and every format's categories in the browser's unit tests.
    if cfg!(target_os = "linux") && lilv() {
        keys.extend(
            scan(&mut shell, bl_test_lv2::bundle())
                .plugins
                .iter()
                .map(plugin_key),
        );
    }
    if cfg!(target_os = "macos") {
        // Apple's own AUDelay is an Audio Unit of subtype `dely`.
        let found = scan(&mut shell, Path::new("/System/Library/Components"));
        assert!(
            found
                .plugins
                .iter()
                .any(|p| plugin_key(p) == "au/aufx/dely/appl")
        );
        keys.push("au/aufx/dely/appl".to_owned());
    }
    assert!(keys.len() >= 2, "{keys:?}");
    for key in &keys {
        let tags = carried(&mut shell, key);
        let found = tags
            .iter()
            .find(|tag| tag.tag == "Function|Delay")
            .unwrap_or_else(|| panic!("{key} carries {tags:?}"));
        assert_eq!(found.source, TagSource::Auto, "{key}");
        assert_eq!(found.origin.as_deref(), Some("plugin"), "{key}");
        // The scanner's own tags are not automatic.
        let plug_in = tags
            .iter()
            .find(|tag| tag.tag == "Content|Plug-In")
            .unwrap();
        assert_eq!(plug_in.source, TagSource::Beatlayer, "{key}");
    }
}

// Spec: browser-tags R50. A category with no row gives no tag, and a feature
// of another mapping gives its own.
#[test]
fn a_plug_in_declaring_reverb_gets_reverb() {
    if !ready() {
        return;
    }
    // SAFETY: nextest runs each test in a process of its own, and the
    // scanner's child, started later, reads this.
    unsafe { std::env::set_var(bl_test_clap::FEATURES_VAR, "reverb") };
    let scratch = Scratch::new("reverb");
    let mut shell = shell_on(&scratch.path("config"));
    let found = scan(&mut shell, bl_test_clap::bundle());
    let key = plugin_key(&found.plugins[0]);
    assert_eq!(auto(&mut shell, &key), ["Function|Reverb"]);
}

/// The 40 generated sounds of the families fixture and the drum loop of the
/// warping tests, copied into a library.
fn library(scratch: &Scratch) -> PathBuf {
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    for entry in std::fs::read_dir(fixtures("families")).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, library.join(path.file_name().unwrap())).unwrap();
    }
    std::fs::copy(
        fixtures("drums").join("drum-loop.wav"),
        library.join("loop-1.wav"),
    )
    .unwrap();
    library
}

/// A shell whose User Library is `library`, indexed, with every sample
/// analysed.
fn analysed(config: &Path, library: &Path) -> Shell {
    let mut shell = shell_on(config);
    shell.set_user_library(Some(library.to_path_buf()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(60)));
    shell.release_analysis(41);
    shell
}

// Spec: browser-tags R49, R53. Generated kicks, snares, hats, basses and
// pads get their tags at least 85 % of the time, and the loop is a loop.
#[test]
fn generated_sounds_get_their_tags_from_their_sound() {
    let scratch = Scratch::new("sounds");
    let library = library(&scratch);
    let mut shell = analysed(&scratch.path("config"), &library);
    for (family, tag) in [
        ("kick", "Function|Kick"),
        ("snare", "Function|Snare"),
        ("hat", "Function|Hi-Hat"),
        ("bass", "Function|Bass"),
        ("pad", "Function|Pad"),
    ] {
        let right = (1..=8)
            .filter(|n| {
                let item = text(&library.join(format!("{family}-{n}.wav")));
                let tags = carried(&mut shell, &item);
                tags.iter().any(|t| {
                    t.tag == tag
                        && t.source == TagSource::Auto
                        && t.origin.as_deref() == Some("sound")
                })
            })
            .count();
        assert!(right as f64 / 8.0 >= 0.85, "{family}: {right} of 8");
    }
    let looped = auto(&mut shell, &text(&library.join("loop-1.wav")));
    assert_eq!(looped, ["Function|Loop"]);
    let one_shot = auto(&mut shell, &text(&library.join("kick-1.wav")));
    assert!(
        one_shot.contains(&"Function|One-Shot".to_owned()),
        "{one_shot:?}"
    );
}

// Spec: browser-tags R51, R54, R56. A tag a person takes off a sample stays
// off after a restart, a rescan and a new analysis, and the tags that
// remain survive a restart in the real database file; nothing is written
// into the file.
#[test]
fn a_tag_taken_off_a_sample_stays_off_and_the_rest_survive_a_restart() {
    let scratch = Scratch::new("removed");
    let config = scratch.path("config");
    let library = library(&scratch);
    let kick = text(&library.join("kick-1.wav"));
    let before = std::fs::read(&kick).unwrap();
    {
        let mut shell = analysed(&config, &library);
        assert!(auto(&mut shell, &kick).contains(&"Function|Kick".to_owned()));
        // Taking an automatic tag off is the tag command of R10.
        run::<TagsState>(
            &mut shell,
            ids::TAG_ITEMS,
            json!({ "items": [kick], "tags": ["Function|Kick"], "attach": false }),
        );
        let left = auto(&mut shell, &kick);
        assert_eq!(left, ["Function|One-Shot"]);
        shell.browser_view().flush_database();
    }
    // The same content with a new modification time is analysed again, and a
    // new shell reads the database back.
    std::fs::write(&kick, &before).unwrap();
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(60)));
    assert_eq!(
        auto(&mut shell, &kick),
        ["Function|One-Shot"],
        "after a restart"
    );
    shell.release_analysis(41);
    assert_eq!(
        auto(&mut shell, &kick),
        ["Function|One-Shot"],
        "after a new analysis"
    );
    // The other samples kept theirs.
    let snare = text(&library.join("snare-1.wav"));
    assert!(auto(&mut shell, &snare).contains(&"Function|Snare".to_owned()));
    // Nothing was written into the file.
    assert_eq!(std::fs::read(&kick).unwrap(), before);
}

// Spec: browser-tags R51, R52, R57. A plug-in's automatic tag a person takes
// off isn't given again by a rescan; making it their own turns it into a
// tag of their own that no rescan changes; a person's edits leave the
// automatic tags as they were.
#[test]
fn a_persons_edits_and_the_automatic_tags_leave_each_other_alone() {
    if !ready() {
        return;
    }
    let scratch = Scratch::new("apart");
    let config = scratch.path("config");
    let key;
    {
        let mut shell = shell_on(&config);
        let found = scan(&mut shell, bl_test_clap::bundle());
        key = plugin_key(&found.plugins[0]);
        run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
        run::<TagsState>(
            &mut shell,
            ids::ADD_TAG,
            json!({ "name": "Wet", "group": "Mine" }),
        );
        // A person's tag doesn't change the automatic ones.
        let before = auto(&mut shell, &key);
        run::<TagsState>(
            &mut shell,
            ids::TAG_ITEMS,
            json!({ "items": [key], "tags": ["Mine|Wet"] }),
        );
        assert_eq!(auto(&mut shell, &key), before);
        let tags = carried(&mut shell, &key);
        let wet = tags.iter().find(|t| t.tag == "Mine|Wet").unwrap();
        assert_eq!(wet.source, TagSource::Own, "apart from the automatic ones");
        // Taking the automatic tag off, a rescan doesn't bring it back.
        run::<TagsState>(
            &mut shell,
            ids::TAG_ITEMS,
            json!({ "items": [key], "tags": ["Function|Delay"], "attach": false }),
        );
        assert!(auto(&mut shell, &key).is_empty());
        let rescanned: PluginScan = run(
            &mut shell,
            plugin_ids::RESCAN,
            json!({ "paths": [text(bl_test_clap::bundle())] }),
        );
        assert_eq!(rescanned.scanned, 1);
        assert!(auto(&mut shell, &key).is_empty(), "after a rescan");
        // And the person's tag is untouched by the automatic ones.
        let tags = carried(&mut shell, &key);
        assert!(tags.iter().any(|t| t.tag == "Mine|Wet"));
        assert!(!tags.iter().any(|t| t.tag == "Function|Delay"));
        shell.browser_view().flush_database();
    }
    // A restart in the same configuration folder keeps both.
    let mut shell = shell_on(&config);
    scan(&mut shell, bl_test_clap::bundle());
    assert!(auto(&mut shell, &key).is_empty(), "after a restart");
    assert!(
        carried(&mut shell, &key)
            .iter()
            .any(|t| t.tag == "Mine|Wet")
    );
}

// Spec: browser-tags R51. A tag a person makes their own from an automatic
// one isn't automatic any more, and stays theirs through a rescan.
#[test]
fn making_an_automatic_tag_ones_own_keeps_it_through_a_rescan() {
    if !ready() {
        return;
    }
    let scratch = Scratch::new("own");
    let mut shell = shell_on(&scratch.path("config"));
    let found = scan(&mut shell, bl_test_clap::bundle());
    let key = plugin_key(&found.plugins[0]);
    assert_eq!(auto(&mut shell, &key), ["Function|Delay"]);
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({ "items": [key], "tags": ["Function|Delay"] }),
    );
    assert!(auto(&mut shell, &key).is_empty());
    run::<PluginScan>(
        &mut shell,
        plugin_ids::RESCAN,
        json!({ "paths": [text(bl_test_clap::bundle())] }),
    );
    let tags = carried(&mut shell, &key);
    let delay = tags.iter().find(|t| t.tag == "Function|Delay").unwrap();
    assert_eq!(delay.source, TagSource::Own);
    // Taking their own tag off leaves it off: it doesn't come back as an
    // automatic one.
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({ "items": [key], "tags": ["Function|Delay"], "attach": false }),
    );
    assert!(
        carried(&mut shell, &key)
            .iter()
            .all(|t| t.tag != "Function|Delay")
    );
}

fn names(state: &BrowserState) -> Vec<String> {
    state.rows.iter().map(|row| row.name.clone()).collect()
}

// Spec: browser-tags R55, R57. Include Auto Tags is on in a new install; off,
// automatic tags neither show in the Filter View nor filter, and `#` search
// doesn't match them, while a person's tags work as before; it is kept
// between launches and read back in `browser.state`.
#[test]
fn include_auto_tags_off_leaves_only_a_persons_tags_filtering() {
    let scratch = Scratch::new("include");
    let config = scratch.path("config");
    let library = library(&scratch);
    let kick = text(&library.join("kick-2.wav"));
    {
        let mut shell = analysed(&config, &library);
        let state: BrowserState = run(&mut shell, ids::STATE, json!({ "limit": 1000 }));
        assert!(state.filter_view.include_auto_tags, "on in a new install");
        assert!(
            state
                .filter_view
                .groups
                .iter()
                .any(|g| g.name == "Function"
                    && g.tags.iter().any(|t| t.full_name == "Function|Kick")),
            "the Filter View shows the Function group's tags in use"
        );
        run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
        run::<TagsState>(
            &mut shell,
            ids::ADD_TAG,
            json!({ "name": "Hard", "group": "Mine" }),
        );
        run::<TagsState>(
            &mut shell,
            ids::TAG_ITEMS,
            json!({ "items": [kick], "tags": ["Mine|Hard"] }),
        );

        let on: BrowserState = run(
            &mut shell,
            ids::FILTER,
            json!({ "tags": ["Function|Kick"], "add": false }),
        );
        let kicks = names(&on);
        assert!(
            kicks.contains(&"kick-2.wav".to_owned()) && kicks.len() >= 7,
            "{kicks:?}"
        );
        run::<BrowserState>(&mut shell, ids::CLEAR_FILTERS, json!({}));
        let searched: BrowserState = run(
            &mut shell,
            ids::SEARCH,
            json!({ "query": "#Function|Snare" }),
        );
        assert!(names(&searched).len() >= 7, "{:?}", names(&searched));
        run::<BrowserState>(&mut shell, ids::CLEAR_FILTERS, json!({}));
        run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "" }));

        let off: FilterViewState = run(
            &mut shell,
            ids::SET_INCLUDE_AUTO_TAGS,
            json!({ "include": false }),
        );
        assert!(!off.include_auto_tags);
        let state: BrowserState = run(&mut shell, ids::STATE, json!({ "limit": 1000 }));
        assert!(!state.filter_view.include_auto_tags, "read back");
        assert!(
            !state
                .filter_view
                .groups
                .iter()
                .any(|g| g.name == "Function"),
            "no automatic tag shows in the Filter View"
        );
        let filtered: BrowserState = run(
            &mut shell,
            ids::FILTER,
            json!({ "tags": ["Function|Kick"], "add": false }),
        );
        assert!(names(&filtered).is_empty(), "{:?}", names(&filtered));
        run::<BrowserState>(&mut shell, ids::CLEAR_FILTERS, json!({}));
        let searched: BrowserState = run(
            &mut shell,
            ids::SEARCH,
            json!({ "query": "#Function|Snare" }),
        );
        assert!(names(&searched).is_empty(), "{:?}", names(&searched));
        // A person's tag works as before, and so do the tags Beatlayer gives.
        let mine: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "#Mine|Hard" }));
        assert_eq!(names(&mine), ["kick-2.wav"]);
        run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "" }));
        let content: BrowserState = run(
            &mut shell,
            ids::FILTER,
            json!({ "tags": ["Content|Sample"], "add": false }),
        );
        assert!(names(&content).len() >= 40);
        // Off, the tags are still on the items.
        assert!(auto(&mut shell, &kick).contains(&"Function|Kick".to_owned()));
        shell.browser_view().flush_database();
    }
    // The setting is kept between launches.
    let mut shell = shell_on(&config);
    let state: BrowserState = run(&mut shell, ids::STATE, json!({ "limit": 1 }));
    assert!(!state.filter_view.include_auto_tags, "after a restart");
    let on: FilterViewState = run(&mut shell, ids::SET_INCLUDE_AUTO_TAGS, json!({}));
    assert!(on.include_auto_tags, "with no argument it toggles");
}
