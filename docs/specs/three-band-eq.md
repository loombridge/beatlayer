# The three-band EQ

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-5.21
- **Parity-matrix row:** Audio effects: EQ Three
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.16 "EQ Three", read 18 October 2026; the crossovers' designs, FreqLo's and FreqHi's ranges and defaults, and the lights' timing are Beatlayer's own

## The behaviour

Triad is Beatlayer's three-band EQ, a DJ-style one: the signal is split into a low, a mid and a high band at two crossover frequencies, FreqLo and FreqHi, each band has its own gain from −∞ dB to +6 dB and its own On switch, and the bands are added again. A switch chooses crossovers of 24 or 48 dB per octave, and a light for each band shows whether that band carries signal.

The crossovers are Linkwitz-Riley filters built from the same second-order sections as Contour's ([EQ spec](eq.md)). At 24 dB they are fourth-order and the low band also passes an all-pass at FreqHi, so the three bands add up flat at any settings. At 48 dB they are eighth-order and the low band is not aligned at FreqHi, so the sum is slightly coloured where the crossovers sit close together, as the manual says of this mode. Triad adds no latency. The framework is the [device framework spec](device-framework.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.16 "EQ Three" | A three-band EQ with independent levels for low, mid and high. Each band's gain runs from −∞ dB to +6 dB, so a band can be removed, and each has an On button. Three lights show signal in each band against an internal threshold of −24 dB, and work while a band is off. FreqLo and FreqHi set the band edges (at 500 Hz and 2 kHz the low band is below 500 Hz, the mid band between, the high band above 2 kHz). A switch makes the crossovers 24 or 48 dB per octave; 48 dB is steeper and costs more processing. The filters are modelled on analog cascades; the 48 dB mode is not perfectly linear and colours the signal slightly even at 0 dB, which 24 dB and EQ Eight do not |
| S2 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter can be automated and modulated |

## Rules

- **R1.** Beatlayer ships Triad, an audio-effect device type of its own name and layout: every parameter has a descriptor and a place in the layout, and the parameters keep their values through a saved set and through the agent API. Gain Low, Gain Mid and Gain High are gains from −∞ dB to +6 dB, at 0 dB at first; On Low, On Mid and On High are switches, on at first; FreqLo runs from 20 Hz to 5 kHz (250 Hz at first) and FreqHi from 200 Hz to 20 kHz (2.5 kHz at first), both Beatlayer's own; Slope is 24 dB at first. (S1, S2)
- **R2.** At 24 dB the crossovers are fourth-order Linkwitz-Riley filters (two Butterworth sections each), the low band passes the all-pass formed by FreqHi's low-pass and high-pass added, and with all bands at 0 dB and on the output has the same size as the input at every frequency, within 0.1 dB, at any FreqLo and FreqHi. FreqHi never acts below FreqLo: when it is set lower, both crossovers sit at FreqLo. (S1)
- **R3.** At 48 dB the crossovers are eighth-order Linkwitz-Riley filters (two fourth-order Butterworth filters each) and the low band is not aligned at FreqHi. The sum of the bands at 0 dB therefore deviates from flat, and the deviation is Beatlayer's stated colouring: under 0.5 dB, and not none, at the defaults (3.3 octaves apart); a dip of 1.5 to 3 dB at FreqLo 500 Hz and FreqHi 2 kHz; and under 0.1 dB with the crossovers more than 7 octaves apart. (S1)
- **R4.** Each crossover falls at its slope on each side: the low band above FreqLo, the mid band below FreqLo and above FreqHi, and the high band below FreqHi fall by 24 dB per octave at 24 dB and 48 dB per octave at 48 dB, within 1 dB, measured away from the crossover and where the response is above −75 dB. (S1)
- **R5.** The output is the sum of each band times its gain, for the bands that are on. A band at −∞ dB or switched off therefore leaves the response of the other two bands, within 0.1 dB, and a gain of +6 dB doubles its band. (S1, S2)
- **R6.** Each band's light is lit while the band's signal, before its gain and switch, is louder than −24 dB: the level is a peak follower with an instant attack and a release time constant of 100 ms, taken on the larger channel. The lights work whether a band is on, off or at −∞ dB. They are published after each block as the readings Low Signal, Mid Signal and High Signal (device framework R35 and R36), 1 and `lit`, or 0 and `off`. (S1)
- **R7.** Triad adds no latency, and its output is the same, bit for bit, at any block size: the crossovers are redesigned from FreqLo and FreqHi every 16 frames counted from the device's start. Changing Slope clears the filters. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/three_band_eq.rs`, `the_three_band_eq_is_described_with_its_ranges_and_defaults`; `crates/shell/tests/eqs_and_dynamics.rs`, `a_set_with_the_four_devices_reopens_with_every_parameter`; `crates/cli/tests/agent.rs`, `an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track`; `rtsan/tests/devices.rs`, `the_three_band_eq_passes_under_rtsan` | unit, MCP script, RTSan |
| R2 | `crates/devices/tests/three_band_eq.rs`, `at_24_db_the_bands_sum_flat_at_any_crossovers` | unit, real-time trap |
| R3 | `crates/devices/tests/three_band_eq.rs`, `at_48_db_the_sum_is_coloured_where_the_crossovers_sit_close` | unit, real-time trap |
| R4 | `crates/devices/tests/three_band_eq.rs`, `each_crossover_falls_at_its_slope` | unit, real-time trap |
| R5 | `crates/devices/tests/three_band_eq.rs`, `a_band_at_minus_infinity_or_off_leaves_the_response_of_the_other_two` | unit, real-time trap |
| R6 | `crates/devices/tests/three_band_eq.rs`, `a_light_is_lit_while_its_band_carries_signal_above_minus_24_db`, `the_display_reads_the_lights` | unit, real-time trap |
| R7 | `crates/devices/tests/three_band_eq.rs`, `renders_are_bit_identical_at_any_block_size_and_add_no_latency` | unit, real-time trap |
| G1, G2 | `crates/devices/tests/rules.rs` over the shipped types (Triad's three gains each act on one band, so the gain rule silences all three together) | real-time trap |
