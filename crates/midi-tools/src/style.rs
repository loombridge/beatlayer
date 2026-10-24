//! Arpeggiator's styles (the arpeggiator spec, R1): the order in which a
//! chord's notes are played, shared by the Arpeggiator device and the
//! Arpeggiate tool. It allocates nothing beyond what the buffers it is given
//! already hold, so the device may call it on the audio thread.

/// The Style choices, in the order of the arpeggiator spec's R1.
pub const STYLE_NAMES: [&str; 18] = [
    "Up",
    "Down",
    "UpDown",
    "DownUp",
    "Up & Down",
    "Down & Up",
    "Converge",
    "Diverge",
    "Con & Diverge",
    "Pinky Up",
    "Pinky UpDown",
    "Thumb Up",
    "Thumb UpDown",
    "Play Order",
    "Chord Trigger",
    "Random",
    "Random Other",
    "Random Once",
];

/// The place of Play Order in [`STYLE_NAMES`].
pub const PLAY_ORDER: usize = 13;
/// The place of Chord Trigger in [`STYLE_NAMES`].
pub const CHORD_TRIGGER: usize = 14;
/// The place of Random in [`STYLE_NAMES`].
pub const RANDOM: usize = 15;
/// The place of Random Other in [`STYLE_NAMES`].
pub const RANDOM_OTHER: usize = 16;
/// The place of Random Once in [`STYLE_NAMES`].
pub const RANDOM_ONCE: usize = 17;

fn converge(order: &mut Vec<usize>, k: usize) {
    let mut low = 0;
    let mut high = k - 1;
    while low <= high {
        order.push(low);
        if low != high {
            order.push(high);
        }
        low += 1;
        high = high.saturating_sub(1);
    }
}

fn diverge(order: &mut Vec<usize>, k: usize) {
    let mid = k / 2;
    if k % 2 == 1 {
        order.push(mid);
        let mut step = 1;
        while mid >= step || mid + step < k {
            if mid >= step {
                order.push(mid - step);
            }
            if mid + step < k {
                order.push(mid + step);
            }
            step += 1;
        }
    } else {
        let mut low = mid - 1;
        let mut high = mid;
        while high < k {
            order.push(low);
            order.push(high);
            low = low.saturating_sub(1);
            high += 1;
        }
    }
}

/// Writes into `order`, which it clears first, the places in a list of `k`
/// held notes (sorted by key, or by the order they were played for Play
/// Order) that `style` plays, in order.
///
/// Chord Trigger, Random and Random Other write the single place 0: the
/// caller picks the note. Random Once shuffles the places with `draw`, which
/// gives a whole number from 0 up to but not including its argument, using
/// `pool` as scratch space; no other style calls `draw`. A style past the
/// list plays Up.
pub fn style_order(
    style: usize,
    k: usize,
    order: &mut Vec<usize>,
    pool: &mut Vec<usize>,
    draw: &mut dyn FnMut(usize) -> usize,
) {
    order.clear();
    if k == 0 {
        return;
    }
    let up = |order: &mut Vec<usize>| order.extend(0..k);
    match style {
        1 => order.extend((0..k).rev()),
        2 => {
            up(order);
            if k > 2 {
                order.extend((1..k - 1).rev());
            }
        }
        3 => {
            order.extend((0..k).rev());
            if k > 2 {
                order.extend(1..k - 1);
            }
        }
        4 => {
            up(order);
            order.extend((0..k).rev());
        }
        5 => {
            order.extend((0..k).rev());
            up(order);
        }
        6 => converge(order, k),
        7 => diverge(order, k),
        8 => {
            converge(order, k);
            diverge(order, k);
        }
        9 | 10 => {
            if k == 1 {
                order.push(0);
            } else {
                let top = k - 1;
                for i in 0..top {
                    order.extend([top, i]);
                }
                if style == 10 {
                    for i in (1..top - 1).rev() {
                        order.extend([top, i]);
                    }
                }
            }
        }
        11 | 12 => {
            if k == 1 {
                order.push(0);
            } else {
                for i in 1..k {
                    order.extend([0, i]);
                }
                if style == 12 {
                    for i in (2..k - 1).rev() {
                        order.extend([0, i]);
                    }
                }
            }
        }
        CHORD_TRIGGER..=RANDOM_OTHER => order.push(0),
        RANDOM_ONCE => {
            pool.clear();
            pool.extend(0..k);
            while !pool.is_empty() {
                let at = draw(pool.len());
                order.push(pool.swap_remove(at));
            }
        }
        _ => up(order),
    }
}
