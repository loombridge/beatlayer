//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, Query, Unobserved};
    vec![
        // A track's switches (mixer-view R7): Enter on Solo or Arm.
        (
            "mixer.solo_exclusive",
            main(nothing, arrangement_track_3, "Solo", Key::Enter),
        ),
        (
            "mixer.arm_exclusive",
            main(nothing, arrangement_track_3, "Arm Recording", Key::Enter),
        ),
        ("mixer_view.state", Exception(Query)),
        (
            "mixer_view.scroll",
            Exception(Equivalent(
                "the wheel; Navigate ▸ Mixer, Left and Right, and the neighbour keys bring the \
                 focused column into view",
            )),
        ),
        // The fold button in a track's title bar (racks spec, R27): Live
        // names the button (25.8) but its key isn't observed yet, and its
        // place in the Tab order with it (the racks spec's open question
        // 19).
        (
            "mixer_view.fold_chains",
            Exception(Unobserved(
                "the fold button in the track's title bar: Live's own key for it isn't observed \
                 yet, and its place in the mixers' Tab order with it",
            )),
        ),
        // A send knob and the In/Out choosers (the routing spec): Up and
        // Down on them.
        (
            "mixer.set_send",
            main(nothing, arrangement_track_3, "A-Return", Key::ArrowUp),
        ),
        (
            "routing.set_input",
            main(nothing, arrangement_track_3, "Input Type", Key::ArrowDown),
        ),
        (
            "routing.set_output",
            main(nothing, arrangement_track_3, "Output Type", Key::ArrowDown),
        ),
        (
            "routing.set_midi_output",
            main(nothing, arrangement_track_1, "Output Type", Key::ArrowUp),
        ),
        // Left and Right on the Monitor switch.
        (
            "routing.set_monitor",
            main(nothing, arrangement_track_3, "Monitoring", Key::ArrowRight),
        ),
        // Main's Cue Out and Main Out (WP-2.13): Up and Down on them step
        // through the interface's output pairs, which need more than one
        // pair to step to.
        (
            "routing.set_cue_out",
            main(
                four_channel_interface,
                session_main,
                "Cue Output",
                Key::ArrowDown,
            ),
        ),
        (
            "routing.set_main_out",
            main(
                four_channel_interface,
                session_main,
                "Main Output",
                Key::ArrowDown,
            ),
        ),
        // A return's Pre/Post switch, in Main's column: Enter switches it.
        (
            "mixer.set_send_tap",
            main(nothing, session_main, "Return A Pre/Post", Key::Enter),
        ),
        // Main's Cue Volume knob (WP-3.18): Up and Down step it.
        (
            "mixer.set_cue_volume",
            main(nothing, session_main, "Cue Volume", Key::ArrowDown),
        ),
        // The Crossfader section (WP-3.17): Main's crossfader and each
        // track's A and B buttons. Live's own key for them and their place
        // in the mixers' Tab order, with the section shown, aren't observed
        // yet; the next observation session records them.
        (
            "mixer.set_crossfader",
            Exception(Unobserved(
                "a drag on Main's crossfader in the Crossfader section",
            )),
        ),
        (
            "mixer.set_crossfade_assign",
            Exception(Unobserved(
                "a click on a track's A or B button in the Crossfader section",
            )),
        ),
        // The Track Options section (WP-5.36): a track's Track Delay, its
        // ms/samples unit switch, and its Keep Monitoring Latency switch,
        // and the pan control's Split Stereo Pan Mode controls (C-PAN-001,
        // C-PAN-002). Live's own keys for them and their place in the
        // mixers' Tab order aren't observed yet.
        (
            "mixer.set_track_delay",
            Exception(Unobserved(
                "a drag or typed value in a track's Track Options",
            )),
        ),
        (
            "mixer.set_pan_mode",
            Exception(Unobserved(
                "a choice from the pan control's context menu (C-PAN-001, C-PAN-002)",
            )),
        ),
        (
            "mixer.set_split_pan_left",
            Exception(Unobserved(
                "a drag on a split pan control in Split Stereo Pan Mode",
            )),
        ),
        (
            "mixer.set_split_pan_right",
            Exception(Unobserved(
                "a drag on a split pan control in Split Stereo Pan Mode",
            )),
        ),
        (
            "mixer.set_keep_monitoring_latency",
            Exception(Unobserved(
                "a click on a track's Keep Monitoring Latency switch",
            )),
        ),
    ]
}
