//! Server-Sent Events, the shape every provider's API streams in.
//!
//! [`events`] reads a response body and yields each event's `data`: its
//! `data:` lines joined with newlines, dispatched at the blank line that
//! ends the event, as the HTML standard says. Comments and the other fields
//! (`event`, `id`, `retry`) are read and dropped: all three APIs put the
//! event's meaning in its data. An event left open when the stream ends is
//! not dispatched.

use std::io::{BufRead, BufReader, Read};

/// The events of an SSE stream, each as its `data` value.
pub(crate) fn events<R: Read>(reader: R) -> Events<R> {
    Events {
        lines: BufReader::new(reader).lines(),
        data: Vec::new(),
    }
}

/// The events of one stream; see [`events`].
pub(crate) struct Events<R: Read> {
    lines: std::io::Lines<BufReader<R>>,
    data: Vec<String>,
}

impl<R: Read> Iterator for Events<R> {
    type Item = std::io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let line = match self.lines.next()? {
                Ok(line) => line,
                Err(error) => return Some(Err(error)),
            };
            if line.is_empty() {
                if self.data.is_empty() {
                    continue;
                }
                return Some(Ok(std::mem::take(&mut self.data).join("\n")));
            }
            if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            }
            // `event`, `id`, `retry`, and `:comment` lines: read and dropped.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &str) -> Vec<String> {
        events(text.as_bytes())
            .collect::<Result<_, _>>()
            .expect("well-formed events")
    }

    #[test]
    fn an_event_is_its_data_lines_joined() {
        let stream = "data: one\ndata: two\n\ndata: three\n\n";
        assert_eq!(all(stream), ["one\ntwo", "three"]);
    }

    #[test]
    fn comments_and_other_fields_are_dropped() {
        let stream = ": keep-alive\nevent: message\nid: 1\ndata: {}\n\n";
        assert_eq!(all(stream), ["{}"]);
    }

    #[test]
    fn an_unfinished_event_when_the_stream_ends_is_not_dispatched() {
        assert_eq!(all("data: lost"), Vec::<String>::new());
    }
}
