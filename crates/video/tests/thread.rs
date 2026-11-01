//! R27: the video thread decodes ahead into a cache and drops what it's done
//! with. Nothing here sleeps or waits for a deadline: `wait_idle` blocks until
//! the thread finishes what it was asked.

use bl_video::{Picture, VideoThread};

use crate::common::*;

#[test]
fn the_thread_decodes_ahead_of_the_time_asked_for() {
    let thread = VideoThread::spawn(movie("mjpeg-24f.mov")).expect("a thread");
    // Nothing decoded before anything's asked.
    assert!(matches!(thread.picture_at(at(0)), Picture::Pending));

    thread.request(at(2));
    thread.wait_idle();
    // The frame asked for and the ones after it are there, without another
    // request.
    for i in 2..8 {
        let Picture::Frame(frame) = thread.picture_at(at(i)) else {
            panic!("frame {i} wasn't decoded ahead");
        };
        assert_eq!(index_of(&frame), i as u32);
    }
    // Frames behind the request are done with.
    assert!(matches!(thread.picture_at(at(0)), Picture::Pending));
}

#[test]
fn a_seek_lands_on_the_right_frame_and_drops_the_old_window() {
    let thread = VideoThread::spawn(movie("mjpeg-24f.mov")).expect("a thread");
    thread.request(at(1));
    thread.wait_idle();
    thread.request(at(20));
    thread.wait_idle();
    let Picture::Frame(frame) = thread.picture_at(at(20)) else {
        panic!("no frame after the seek");
    };
    assert_eq!(index_of(&frame), 20);
    assert!(matches!(thread.picture_at(at(1)), Picture::Pending));
    // Going back works too.
    thread.request(at(4));
    thread.wait_idle();
    let Picture::Frame(frame) = thread.picture_at(at(4)) else {
        panic!("no frame after seeking back");
    };
    assert_eq!(index_of(&frame), 4);
}

#[test]
fn gaps_are_black_without_decoding_and_the_frame_after_one_is_ready() {
    let thread = VideoThread::spawn(movie("mjpeg-gaps.mov")).expect("a thread");
    thread.request(at(9));
    thread.wait_idle();
    assert!(matches!(thread.picture_at(at(9)), Picture::Black));
    // The first frame after the gap is decoded before the playhead gets there.
    let Picture::Frame(frame) = thread.picture_at(at(12)) else {
        panic!("the frame after the gap wasn't ready");
    };
    assert_eq!(index_of(&frame), 8);
}

#[test]
fn dropping_the_thread_stops_it() {
    let thread = VideoThread::spawn(movie("mjpeg-24f.mov")).expect("a thread");
    thread.request(at(3));
    drop(thread);
}
