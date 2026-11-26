//! Frames for the UI: coalescing, splitting, and sequence numbers.
//!
//! Agents stream text a few characters at a time. Sending every delta as its own Tauri message would
//! flood the webview, so drivers push events into a [`FrameBatcher`] and flush it about once per
//! animation frame. A flush merges runs of deltas for the same message or thought, splits text that
//! does not fit, and packs everything into [`EventBatch`]es whose JSON stays below the frame limit
//! ([`MAX_FRAME_BYTES`] unless overridden), the size Tauri still evaluates inline instead of taking its
//! slower fetch path.
//!
//! Sequence numbers are assigned at flush time, after merging and splitting, because those are the
//! events the UI actually receives: its gap detection only works if every delivered event has exactly
//! one number and the numbers never skip. Stored rows carry the same numbers.
//!
//! The session actor also needs to know which turn each delivered event belongs to, to store it
//! under that turn. [`FrameBatcher::push_tagged`] carries a turn tag with each event and
//! [`FrameBatcher::flush_tagged`] returns the tags beside the frames: only adjacent deltas with equal
//! tags merge, and the pieces of a split delta keep its tag.

use std::io;
use std::sync::Arc;

use ghost_protocol::{EventBatch, GhostEvent, MAX_FRAME_BYTES};
use unicode_segmentation::GraphemeCursor;

/// The turn an event belongs to, or `None` for an event outside any turn.
pub type TurnTag = Option<Arc<str>>;

/// Buffers one chat's events between flushes and packs them into size-limited frames.
#[derive(Debug, Clone)]
pub struct FrameBatcher {
    chat_id: String,
    max_bytes: usize,
    next_seq: u32,
    pending: Vec<(GhostEvent, TurnTag)>,
}

impl FrameBatcher {
    /// A batcher for `chat_id` whose frames stay below [`MAX_FRAME_BYTES`].
    pub fn new(chat_id: impl Into<String>) -> Self {
        Self::with_max_bytes(chat_id, MAX_FRAME_BYTES)
    }

    /// A batcher whose frames stay below `max_bytes` instead (tests use small limits to exercise
    /// splitting without megabytes of input).
    pub fn with_max_bytes(chat_id: impl Into<String>, max_bytes: usize) -> Self {
        Self {
            chat_id: chat_id.into(),
            max_bytes,
            next_seq: 0,
            pending: Vec::new(),
        }
    }

    /// Continues numbering at `next_seq`, for a chat resumed from storage, so the UI's gap
    /// detection keeps working across app restarts.
    #[must_use]
    pub fn starting_at(mut self, next_seq: u32) -> Self {
        self.next_seq = next_seq;
        self
    }

    /// Queues `event` for the next flush.
    ///
    /// A text delta that directly follows a delta for the same message (or the same thought, or the
    /// same user message) is appended to it instead, which keeps a fast stream down to one event per
    /// frame. Any other event in between starts a new run, so the order the agent produced is
    /// preserved exactly.
    pub fn push(&mut self, event: GhostEvent) {
        self.push_tagged(event, None);
    }

    /// [`push`](Self::push) with the turn the event belongs to. Deltas merge only when their tags are
    /// equal too, so an untagged late update never joins a tagged run.
    pub fn push_tagged(&mut self, event: GhostEvent, turn: TurnTag) {
        if let Some((last, last_turn)) = self.pending.last_mut()
            && *last_turn == turn
            && let Some((open_kind, open_id, buffer)) = delta_mut(last)
            && let Some((kind, id, text)) = delta_ref(&event)
            && open_kind == kind
            && open_id == id
        {
            buffer.push_str(text);
            return;
        }
        self.pending.push((event, turn));
    }

    /// Drains everything queued into frames, in order, each one's [`EventBatch::encoded_len`] below
    /// the frame limit.
    ///
    /// Events are packed greedily: one that does not fit in the current frame starts the next. A text
    /// delta too large for any frame is split across frames on grapheme boundaries (char boundaries
    /// when a single grapheme cluster is larger than a frame), and every piece becomes an event with
    /// its own sequence number. Any other event too large for an empty frame cannot be split, so it
    /// goes out alone in an oversized frame and a warning is logged. Returns an empty vec when nothing
    /// is queued.
    #[must_use = "flushed events are no longer in the batcher; dropping the frames loses them"]
    pub fn flush(&mut self) -> Vec<EventBatch> {
        self.flush_tagged()
            .into_iter()
            .map(|(frame, _)| frame)
            .collect()
    }

    /// [`flush`](Self::flush), with each frame's turn tags: `tags[i]` belongs to `events[i]`. The
    /// pieces of a split delta keep its tag.
    #[must_use = "flushed events are no longer in the batcher; dropping the frames loses them"]
    pub fn flush_tagged(&mut self) -> Vec<(EventBatch, Vec<TurnTag>)> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let mut packer = Packer::new(&self.chat_id, self.max_bytes, self.next_seq);
        for (event, turn) in self.pending.drain(..) {
            packer.add(event, turn);
        }
        let (frames, next_seq) = packer.finish();
        self.next_seq = next_seq;
        frames
    }

    /// Whether nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The sequence number the next flushed event will get.
    pub fn next_seq(&self) -> u32 {
        self.next_seq
    }

    /// Number of queued events, after coalescing.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

/// Which kind of streamed text a delta carries. Every kind coalesces and splits the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeltaKind {
    Message,
    Thought,
    User,
}

impl DeltaKind {
    pub(crate) fn event(self, id: String, text: String) -> GhostEvent {
        match self {
            Self::Message => GhostEvent::MessageDelta {
                message_id: id,
                text,
            },
            Self::Thought => GhostEvent::ThoughtDelta {
                thought_id: id,
                text,
            },
            Self::User => GhostEvent::UserMessageDelta {
                message_id: id,
                text,
            },
        }
    }
}

/// A delta's kind, id and text, or `None` for any other event.
fn delta_ref(event: &GhostEvent) -> Option<(DeltaKind, &str, &str)> {
    match event {
        GhostEvent::MessageDelta { message_id, text } => {
            Some((DeltaKind::Message, message_id, text))
        }
        GhostEvent::ThoughtDelta { thought_id, text } => {
            Some((DeltaKind::Thought, thought_id, text))
        }
        GhostEvent::UserMessageDelta { message_id, text } => {
            Some((DeltaKind::User, message_id, text))
        }
        _ => None,
    }
}

/// [`delta_ref`] with the text open for appending.
fn delta_mut(event: &mut GhostEvent) -> Option<(DeltaKind, &str, &mut String)> {
    match event {
        GhostEvent::MessageDelta { message_id, text } => {
            Some((DeltaKind::Message, message_id, text))
        }
        GhostEvent::ThoughtDelta { thought_id, text } => {
            Some((DeltaKind::Thought, thought_id, text))
        }
        GhostEvent::UserMessageDelta { message_id, text } => {
            Some((DeltaKind::User, message_id, text))
        }
        _ => None,
    }
}

/// Builds one flush's frames in order while tracking the open frame's exact JSON length, so packing is
/// linear in the bytes flushed instead of re-serializing the frame for every event added.
struct Packer<'a> {
    chat_id: &'a str,
    /// Largest encoded length a frame may have: frames must stay strictly below `max_bytes`.
    limit: usize,
    /// Encoded length of a frame with no events, minus the digits of its `seq_start`.
    overhead: usize,
    next_seq: u32,
    frames: Vec<(EventBatch, Vec<TurnTag>)>,
    /// Events of the open frame.
    events: Vec<GhostEvent>,
    /// Their turn tags.
    tags: Vec<TurnTag>,
    seq_start: u32,
    /// Encoded length of the open frame with its current events.
    len: usize,
}

impl<'a> Packer<'a> {
    fn new(chat_id: &'a str, max_bytes: usize, next_seq: u32) -> Self {
        let empty = EventBatch {
            chat_id: chat_id.to_owned(),
            seq_start: 0,
            events: Vec::new(),
        };
        // `seq_start: 0` contributes one digit; each frame adds the digits of its real `seq_start`.
        let overhead = empty.encoded_len().saturating_sub(1);
        Self {
            chat_id,
            limit: max_bytes.saturating_sub(1),
            overhead,
            next_seq,
            frames: Vec::new(),
            events: Vec::new(),
            tags: Vec::new(),
            seq_start: next_seq,
            len: overhead.saturating_add(decimal_digits(next_seq)),
        }
    }

    fn add(&mut self, event: GhostEvent, tag: TurnTag) {
        match event {
            GhostEvent::MessageDelta { message_id, text } => {
                self.add_delta(DeltaKind::Message, message_id, text, tag)
            }
            GhostEvent::ThoughtDelta { thought_id, text } => {
                self.add_delta(DeltaKind::Thought, thought_id, text, tag)
            }
            GhostEvent::UserMessageDelta { message_id, text } => {
                self.add_delta(DeltaKind::User, message_id, text, tag)
            }
            other => self.add_whole(other, tag),
        }
    }

    /// Bytes left in the open frame for one more event, after the comma that would precede it.
    fn room(&self) -> usize {
        let separator = usize::from(!self.events.is_empty());
        self.limit
            .saturating_sub(self.len.saturating_add(separator))
    }

    /// Whether a delta of JSON length `base` plus the escaped `text` fits in an empty frame (the next
    /// one, whose `seq_start` is `next_seq`).
    fn fits_alone(&self, base: usize, text: &str) -> bool {
        let empty = self.overhead.saturating_add(decimal_digits(self.next_seq));
        self.limit
            .saturating_sub(empty)
            .checked_sub(base)
            .is_some_and(|budget| fit_prefix(text, budget).0 == text.len())
    }

    fn push_event(&mut self, event: GhostEvent, size: usize, tag: TurnTag) {
        let separator = usize::from(!self.events.is_empty());
        if self.events.is_empty() {
            self.seq_start = self.next_seq;
        }
        self.len = self.len.saturating_add(separator).saturating_add(size);
        self.events.push(event);
        self.tags.push(tag);
        // Wrapping instead of overflowing: four billion events in one chat is out of reach, and an
        // overflow panic would take the whole session down with it.
        self.next_seq = self.next_seq.wrapping_add(1);
    }

    fn close_frame(&mut self) {
        if self.events.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.events);
        let tags = std::mem::take(&mut self.tags);
        self.frames.push((
            EventBatch {
                chat_id: self.chat_id.to_owned(),
                seq_start: self.seq_start,
                events,
            },
            tags,
        ));
        self.len = self.overhead.saturating_add(decimal_digits(self.next_seq));
    }

    fn finish(mut self) -> (Vec<(EventBatch, Vec<TurnTag>)>, u32) {
        self.close_frame();
        (self.frames, self.next_seq)
    }

    /// Adds an event that cannot be split.
    fn add_whole(&mut self, event: GhostEvent, tag: TurnTag) {
        let size = event_len(&event);
        if size > self.room() && !self.events.is_empty() {
            self.close_frame();
        }
        if size > self.room() {
            tracing::warn!(
                event = %event_type(&event),
                size,
                max_bytes = self.limit.saturating_add(1),
                "event is larger than a frame on its own; sending it alone"
            );
            self.push_event(event, size, tag);
            self.close_frame();
        } else {
            self.push_event(event, size, tag);
        }
    }

    /// Adds a text delta, splitting its text across as many frames as it needs. Every piece keeps
    /// `tag`.
    fn add_delta(&mut self, kind: DeltaKind, id: String, mut text: String, tag: TurnTag) {
        // The JSON length of this delta with empty text. A piece of text adds exactly its escaped length
        // on top, so pieces can be sized without serializing them.
        let base = event_len(&kind.event(id.clone(), String::new()));
        let mut start = 0;
        loop {
            // Only reachable after a split, so an originally empty delta still goes out once.
            if start > 0 && start >= text.len() {
                return;
            }
            let rest = text.get(start..).unwrap_or_default();
            let Some(budget) = self.room().checked_sub(base) else {
                if !self.events.is_empty() {
                    self.close_frame();
                    continue;
                }
                // Not even an empty delta fits in an empty frame (an absurdly long id or a tiny limit),
                // so splitting cannot help: the remainder goes out whole, like any oversized event.
                let size = base.saturating_add(escaped_str_len(rest));
                tracing::warn!(
                    size,
                    max_bytes = self.limit.saturating_add(1),
                    "delta id alone exceeds the frame limit; sending the text unsplit"
                );
                let piece = take_from(&mut text, start);
                self.push_event(kind.event(id, piece), size, tag);
                self.close_frame();
                return;
            };
            let (end, escaped) = fit_prefix(rest, budget);
            if end == rest.len() {
                let size = base.saturating_add(escaped);
                let piece = take_from(&mut text, start);
                self.push_event(kind.event(id, piece), size, tag);
                return;
            }
            // A delta that fits in a frame of its own is never split: it moves to the next frame whole, so
            // a run of text stays one event. A delta too large for any frame must be split anyway; it
            // starts in the space left here, which never needs more frames than starting afresh and
            // sometimes needs one fewer.
            if !self.events.is_empty() && self.fits_alone(base, rest) {
                self.close_frame();
                continue;
            }
            // Cut at the last grapheme boundary that still fits.
            let cut = grapheme_floor(rest, end);
            if cut > 0 {
                let dropped = escaped_str_len(rest.get(cut..end).unwrap_or_default());
                let size = base.saturating_add(escaped.saturating_sub(dropped));
                let piece = rest.get(..cut).unwrap_or_default().to_owned();
                self.push_event(kind.event(id.clone(), piece), size, tag.clone());
                self.close_frame();
                start += cut;
                continue;
            }
            if !self.events.is_empty() {
                // The first cluster may still fit in an empty frame.
                self.close_frame();
                continue;
            }
            // An empty frame and still no grapheme boundary fits: one cluster is larger than a whole
            // frame (or the limit is tiny). Char boundaries are the hard requirement, so cut there; if
            // not even one char fits, send one char over the limit rather than looping forever.
            let cut = if end > 0 {
                end
            } else {
                rest.chars().next().map_or(rest.len(), char::len_utf8)
            };
            let piece = rest.get(..cut).unwrap_or_default();
            let size = base.saturating_add(escaped_str_len(piece));
            if size > self.room() {
                tracing::warn!(
                    size,
                    max_bytes = self.limit.saturating_add(1),
                    "frame limit is smaller than one character of this delta"
                );
            }
            let piece = piece.to_owned();
            self.push_event(kind.event(id.clone(), piece), size, tag.clone());
            self.close_frame();
            start += cut;
        }
    }
}

/// The tail of `text` from byte `start`, moving the string instead of copying it when nothing was split
/// off its front.
fn take_from(text: &mut String, start: usize) -> String {
    if start == 0 {
        std::mem::take(text)
    } else {
        text.get(start..).unwrap_or_default().to_owned()
    }
}

/// Counts bytes instead of storing them, so measuring an event allocates nothing.
struct ByteCount(usize);

impl io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialized size of one event inside a frame. Serializing these types cannot fail; if it ever did,
/// the event counts as too large to share a frame, so it cannot spoil the events around it.
fn event_len(event: &GhostEvent) -> usize {
    let mut count = ByteCount(0);
    match serde_json::to_writer(&mut count, event) {
        Ok(()) => count.0,
        Err(_) => usize::MAX,
    }
}

/// The event's wire tag, for logs.
fn event_type(event: &GhostEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Bytes `c` takes inside a JSON string the way serde_json writes it: two-byte escapes for quotes,
/// backslashes and the common control characters, `\u00XX` for the other control characters, raw
/// UTF-8 for everything else (serde_json leaves non-ASCII unescaped).
fn escaped_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{08}' | '\t' | '\n' | '\u{0c}' | '\r' => 2,
        '\u{00}'..='\u{1f}' => 6,
        _ => c.len_utf8(),
    }
}

fn escaped_str_len(text: &str) -> usize {
    text.chars().map(escaped_len).fold(0, usize::saturating_add)
}

/// The longest prefix of `text` that ends on a char boundary and escapes to at most `budget` bytes, as
/// `(byte length, escaped length)`. Stops scanning at the budget, so it is linear in the prefix only.
fn fit_prefix(text: &str, budget: usize) -> (usize, usize) {
    let mut escaped = 0usize;
    for (index, c) in text.char_indices() {
        let next = escaped.saturating_add(escaped_len(c));
        if next > budget {
            return (index, escaped);
        }
        escaped = next;
    }
    (text.len(), escaped)
}

/// The last grapheme boundary of `text` at or before the char boundary `at`, or 0 when the cluster
/// containing `at` starts the text. Keeps an emoji sequence, a flag, or a letter with its accents in one
/// piece, so the UI never renders half a cluster between two frames.
fn grapheme_floor(text: &str, at: usize) -> usize {
    if at == 0 || at >= text.len() {
        return at.min(text.len());
    }
    let mut cursor = GraphemeCursor::new(at, text.len(), true);
    match cursor.is_boundary(text, 0) {
        Ok(true) => at,
        Ok(false) => cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0),
        // Only possible with partial context; the whole text is given. A char boundary is still valid.
        Err(_) => at,
    }
}

fn decimal_digits(n: u32) -> usize {
    n.checked_ilog10().map_or(1, |digits| digits as usize + 1)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ghost_protocol::{NoticeLevel, ToolContent, ToolStatus, ToolUpdate};
    use unicode_segmentation::UnicodeSegmentation;

    use super::*;

    fn delta(id: &str, text: &str) -> GhostEvent {
        GhostEvent::MessageDelta {
            message_id: id.into(),
            text: text.into(),
        }
    }

    fn thought(id: &str, text: &str) -> GhostEvent {
        GhostEvent::ThoughtDelta {
            thought_id: id.into(),
            text: text.into(),
        }
    }

    fn notice(message: &str) -> GhostEvent {
        GhostEvent::Notice {
            level: NoticeLevel::Info,
            message: message.into(),
        }
    }

    fn all_events(frames: &[EventBatch]) -> Vec<GhostEvent> {
        frames
            .iter()
            .flat_map(|frame| frame.events.iter().cloned())
            .collect()
    }

    /// Asserts that `frames` are non-empty and number their events contiguously from `first`; returns
    /// the number the next event would get.
    fn assert_contiguous(frames: &[EventBatch], first: u32) -> u32 {
        let mut expected = first;
        for frame in frames {
            assert_eq!(
                frame.seq_start, expected,
                "frames must continue each other's numbering"
            );
            assert!(!frame.events.is_empty(), "frames are never empty");
            expected += u32::try_from(frame.events.len()).expect("frame length fits u32");
        }
        expected
    }

    /// Splits `text` every `chars` characters (not graphemes), the way an agent's deltas arrive.
    fn char_chunks(text: &str, chars: usize) -> Vec<&str> {
        let mut chunks = Vec::new();
        let mut start = 0;
        for (count, (index, _)) in text.char_indices().enumerate() {
            if count > 0 && count % chars == 0 {
                chunks.push(&text[start..index]);
                start = index;
            }
        }
        chunks.push(&text[start..]);
        chunks
    }

    /// Text of every delta for `id`, in order, plus the byte offsets where pieces end.
    fn delta_text(frames: &[EventBatch], id: &str) -> (String, Vec<usize>) {
        let mut text = String::new();
        let mut cuts = Vec::new();
        for event in all_events(frames) {
            if let GhostEvent::MessageDelta {
                message_id,
                text: piece,
            } = event
            {
                assert_eq!(message_id, id);
                text.push_str(&piece);
                cuts.push(text.len());
            }
        }
        (text, cuts)
    }

    #[test]
    fn coalesces_consecutive_deltas_for_the_same_id() {
        let mut batcher = FrameBatcher::new("s");
        batcher.push(delta("m1", "Hel"));
        batcher.push(delta("m1", "lo"));
        batcher.push(delta("m1", ", world"));
        batcher.push(thought("t1", "hm"));
        batcher.push(thought("t1", "m"));
        assert_eq!(batcher.pending_len(), 2);
        let frames = batcher.flush();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].events,
            vec![delta("m1", "Hello, world"), thought("t1", "hmm")]
        );
        assert_eq!(frames[0].seq_start, 0);
        assert_eq!(batcher.next_seq(), 2);
    }

    #[test]
    fn does_not_coalesce_across_other_events_ids_or_kinds() {
        let mut batcher = FrameBatcher::new("s");
        batcher.push(delta("m1", "a"));
        batcher.push(delta("m2", "b"));
        batcher.push(delta("m1", "c"));
        batcher.push(notice("between"));
        batcher.push(delta("m1", "d"));
        batcher.push(thought("m1", "e"));
        batcher.push(GhostEvent::MessageCompleted {
            message_id: "m1".into(),
        });
        batcher.push(delta("m1", "f"));
        batcher.push(thought("t1", "g"));
        batcher.push(thought("t2", "h"));
        let events = all_events(&batcher.flush());
        assert_eq!(
            events,
            vec![
                delta("m1", "a"),
                delta("m2", "b"),
                delta("m1", "c"),
                notice("between"),
                delta("m1", "d"),
                thought("m1", "e"),
                GhostEvent::MessageCompleted {
                    message_id: "m1".into()
                },
                delta("m1", "f"),
                thought("t1", "g"),
                thought("t2", "h"),
            ]
        );
    }

    #[test]
    fn does_not_coalesce_across_flushes() {
        let mut batcher = FrameBatcher::new("s");
        batcher.push(delta("m", "a"));
        let first = batcher.flush();
        batcher.push(delta("m", "b"));
        let second = batcher.flush();
        assert_eq!(first[0].events, vec![delta("m", "a")]);
        assert_eq!(first[0].seq_start, 0);
        assert_eq!(second[0].events, vec![delta("m", "b")]);
        assert_eq!(second[0].seq_start, 1);
    }

    #[test]
    fn empty_deltas_are_kept_unless_they_coalesce() {
        let mut batcher = FrameBatcher::new("s");
        batcher.push(delta("m", ""));
        batcher.push(notice("x"));
        batcher.push(delta("m", "a"));
        batcher.push(delta("m", ""));
        assert_eq!(
            all_events(&batcher.flush()),
            vec![delta("m", ""), notice("x"), delta("m", "a")]
        );
    }

    #[test]
    fn sequence_numbers_are_contiguous_across_flushes() {
        let mut batcher = FrameBatcher::with_max_bytes("s", 256);
        let mut next = 0;
        for round in 0..6 {
            for index in 0..(round * 7 + 1) {
                batcher.push(notice(&format!("{round}-{index}")));
            }
            // Later rounds also need their delta split, and every piece takes a number.
            batcher.push(delta("m", &"x".repeat(300 * round)));
            batcher.push(GhostEvent::MessageCompleted {
                message_id: "m".into(),
            });
            let frames = batcher.flush();
            assert!(frames.iter().all(|frame| frame.encoded_len() < 256));
            next = assert_contiguous(&frames, next);
            assert_eq!(batcher.next_seq(), next);
            assert!(batcher.is_empty());
        }
        assert!(
            next > 100,
            "the rounds produced split pieces and many frames"
        );
        // Flushing with nothing queued yields nothing and keeps the counter.
        assert!(batcher.flush().is_empty());
        assert_eq!(batcher.next_seq(), next);
    }

    #[test]
    fn starting_at_continues_numbering() {
        let mut batcher = FrameBatcher::new("s").starting_at(41);
        assert_eq!(batcher.next_seq(), 41);
        batcher.push(notice("a"));
        batcher.push(notice("b"));
        let frames = batcher.flush();
        assert_eq!(frames[0].seq_start, 41);
        assert_eq!(batcher.next_seq(), 43);
    }

    #[test]
    fn sequence_numbers_wrap_instead_of_panicking() {
        let mut batcher = FrameBatcher::new("s").starting_at(u32::MAX);
        batcher.push(notice("last"));
        batcher.push(delta("m", "first again"));
        let frames = batcher.flush();
        assert_eq!(frames[0].seq_start, u32::MAX);
        assert_eq!(batcher.next_seq(), 1);
    }

    #[test]
    fn small_events_share_frames_greedily() {
        let mut batcher = FrameBatcher::with_max_bytes("s", 512);
        for index in 0..200 {
            batcher.push(notice(&format!("notice number {index}")));
        }
        let frames = batcher.flush();
        assert!(frames.len() > 5);
        assert_contiguous(&frames, 0);
        for pair in frames.windows(2) {
            let [frame, next] = pair else { unreachable!() };
            assert!(frame.encoded_len() < 512);
            // Each frame is full: the next frame's first event would not have fit after the comma.
            let first = next.events.first().expect("frames are never empty");
            assert!(frame.encoded_len() + 1 + event_len(first) >= 512);
        }
        assert_eq!(all_events(&frames).len(), 200);
    }

    #[test]
    fn a_delta_that_fits_alone_moves_to_the_next_frame_whole() {
        let mut batcher = FrameBatcher::with_max_bytes("s", 512);
        batcher.push(notice(&"n".repeat(350)));
        batcher.push(delta("m", &"d".repeat(200)));
        let frames = batcher.flush();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].events, vec![notice(&"n".repeat(350))]);
        assert_eq!(frames[1].events, vec![delta("m", &"d".repeat(200))]);
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn fitting_alone_is_exact_at_the_boundary() {
        let max_bytes = 512;
        let frame_with = |text: &str| {
            let frame = EventBatch {
                chat_id: "s".into(),
                seq_start: 1,
                events: vec![delta("m", text)],
            };
            frame.encoded_len()
        };
        // A delta that alone makes a frame of exactly `max_bytes - 1` still fits; one byte more does not.
        let fitting = "d".repeat(max_bytes - 1 - frame_with(""));
        assert_eq!(frame_with(&fitting), max_bytes - 1);
        let over = format!("{fitting}d");

        // Exactly fitting: moves to the next frame whole.
        let mut batcher = FrameBatcher::with_max_bytes("s", max_bytes);
        batcher.push(notice("first"));
        batcher.push(delta("m", &fitting));
        let frames = batcher.flush();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].events, vec![delta("m", &fitting)]);
        assert_eq!(frames[1].encoded_len(), max_bytes - 1);

        // One byte over: split, starting in the space left after the notice.
        let mut batcher = FrameBatcher::with_max_bytes("s", max_bytes);
        batcher.push(notice("first"));
        batcher.push(delta("m", &over));
        let frames = batcher.flush();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].events.len(), 2);
        assert_eq!(delta_text(&frames, "m").0, over);
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn split_frames_are_filled_to_the_byte() {
        // Plain ASCII escapes to itself, so a split frame must land exactly on the limit minus one:
        // this pins the size bookkeeping to the real serializer.
        let text = "abcdefghij".repeat(2_000);
        let mut batcher = FrameBatcher::with_max_bytes("session", 500);
        batcher.push(notice("first"));
        batcher.push(delta("m", &text));
        let frames = batcher.flush();
        let (last, full) = frames.split_last().expect("several frames");
        assert!(full.len() > 30);
        for frame in full {
            assert_eq!(frame.encoded_len(), 499);
        }
        assert!(last.encoded_len() < 500);
        // The first piece shares a frame with the notice before it.
        assert_eq!(frames[0].events.len(), 2);
        assert_eq!(delta_text(&frames, "m").0, text);
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn mixed_unicode_delta_of_100_kb_splits_under_the_limit() {
        let pattern = "Plain ASCII, déjà vu, naïve café, Ångström, 日本語のテキスト, 한국어 문장, \
                       Ελληνικά, emoji 🚀🎉👍🏽, family 👨‍👩‍👧‍👦, flags 🇸🇮🇺🇸, e\u{301}, \"quotes\", \
                       \\backslashes\\, tab\tand CRLF\r\n, control \u{1} char. ";
        let mut text = String::new();
        while text.len() < 100 * 1024 {
            text.push_str(pattern);
        }
        let boundaries: HashSet<usize> = text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .chain([text.len()])
            .collect();
        for max_bytes in [MAX_FRAME_BYTES, 1024, 300] {
            let mut batcher = FrameBatcher::with_max_bytes("session-1", max_bytes);
            // Pushed as many small deltas, so coalescing first builds the 100 KB delta.
            for chunk in char_chunks(&text, 37) {
                batcher.push(delta("m", chunk));
            }
            assert_eq!(batcher.pending_len(), 1);
            let frames = batcher.flush();
            assert!(
                frames.len() > 10,
                "{max_bytes}: expected many frames, got {}",
                frames.len()
            );
            for frame in &frames {
                assert!(
                    frame.encoded_len() < max_bytes,
                    "{max_bytes}: frame of {} bytes",
                    frame.encoded_len()
                );
            }
            assert_contiguous(&frames, 0);
            let (rebuilt, cuts) = delta_text(&frames, "m");
            assert_eq!(
                rebuilt, text,
                "{max_bytes}: pieces must concatenate to the original"
            );
            // Every cluster here is far smaller than a frame, so no cut may land inside one.
            for cut in cuts {
                assert!(
                    boundaries.contains(&cut),
                    "{max_bytes}: cut at {cut} splits a grapheme cluster"
                );
            }
        }
    }

    #[test]
    fn cut_backs_off_to_the_start_of_an_emoji_sequence() {
        let family = "👨‍👩‍👧‍👦";
        let max_bytes = 400;
        let empty = EventBatch {
            chat_id: "s".into(),
            seq_start: 0,
            events: vec![delta("m", "")],
        };
        let budget = max_bytes - 1 - empty.encoded_len();
        // The family emoji (25 bytes) starts 10 bytes before the budget runs out.
        let lead = "a".repeat(budget - 10);
        let text = format!("{lead}{family}{}", "b".repeat(100));
        let mut batcher = FrameBatcher::with_max_bytes("s", max_bytes);
        batcher.push(delta("m", &text));
        let frames = batcher.flush();
        assert_eq!(frames[0].events, vec![delta("m", &lead)]);
        match frames[1].events.first() {
            Some(GhostEvent::MessageDelta { text, .. }) => assert!(text.starts_with(family)),
            other => panic!("expected the rest of the delta, got {other:?}"),
        }
        assert_eq!(delta_text(&frames, "m").0, text);
    }

    #[test]
    fn a_cluster_larger_than_a_frame_splits_on_char_boundaries() {
        // One grapheme cluster of about 1 KB: a letter followed by 500 combining accents.
        let text = format!("x e{}", "\u{301}".repeat(500));
        let mut batcher = FrameBatcher::with_max_bytes("s", 300);
        batcher.push(notice("before"));
        batcher.push(delta("m", &text));
        let frames = batcher.flush();
        assert!(frames.len() > 4);
        for frame in &frames {
            assert!(frame.encoded_len() < 300);
        }
        assert_eq!(delta_text(&frames, "m").0, text);
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn oversized_non_delta_event_goes_alone() {
        let big = GhostEvent::ToolUpdate(ToolUpdate {
            tool_call_id: "tool-1".into(),
            status: Some(ToolStatus::Completed),
            title: None,
            kind: None,
            locations: None,
            raw_input: None,
            tool_name: None,
            content: Some(vec![ToolContent::Text {
                text: "x".repeat(2_000),
                truncated: None,
            }]),
            truncated: None,
        });
        let mut batcher = FrameBatcher::with_max_bytes("s", 512);
        batcher.push(delta("m", "before"));
        batcher.push(big.clone());
        batcher.push(notice("after"));
        let frames = batcher.flush();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].events, vec![delta("m", "before")]);
        assert_eq!(frames[1].events, vec![big]);
        assert!(frames[1].encoded_len() >= 512);
        assert_eq!(frames[2].events, vec![notice("after")]);
        assert!(frames[0].encoded_len() < 512 && frames[2].encoded_len() < 512);
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn an_id_too_long_for_any_frame_sends_its_text_unsplit() {
        let long_id = "i".repeat(600);
        let mut batcher = FrameBatcher::with_max_bytes("s", 512);
        batcher.push(notice("before"));
        batcher.push(delta(&long_id, &"t".repeat(5_000)));
        batcher.push(notice("after"));
        let frames = batcher.flush();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1].events, vec![delta(&long_id, &"t".repeat(5_000))]);
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn degenerate_limits_terminate_without_losing_text() {
        let text = "héllo 🚀 wörld \"x\" \u{1}".repeat(3);
        for max_bytes in [0, 1, 60, 85, 90, 100, usize::MAX] {
            let mut batcher = FrameBatcher::with_max_bytes("s", max_bytes);
            batcher.push(notice("a"));
            batcher.push(delta("m", &text));
            batcher.push(thought("t", "thinking"));
            let frames = batcher.flush();
            assert_eq!(delta_text(&frames, "m").0, text, "limit {max_bytes}");
            let next = assert_contiguous(&frames, 0);
            assert_eq!(batcher.next_seq(), next);
        }
    }

    #[test]
    fn escaped_len_matches_serde_json_for_every_char() {
        for c in (0u8..=0x7f).map(char::from) {
            let json = serde_json::to_string(&c.to_string()).expect("serialize");
            assert_eq!(escaped_len(c), json.len() - 2, "{c:?}");
        }
        let chars: Vec<char> = (0..=0x10_FFFF_u32).filter_map(char::from_u32).collect();
        for chunk in chars.chunks(4096) {
            let text: String = chunk.iter().collect();
            let json = serde_json::to_string(&text).expect("serialize");
            assert_eq!(escaped_str_len(&text), json.len() - 2);
        }
    }

    #[test]
    fn grapheme_floor_respects_clusters() {
        let text = "ab👍🏽cd";
        let thumbs = text.find('👍').expect("present");
        let modifier = thumbs + '👍'.len_utf8();
        assert_eq!(grapheme_floor(text, modifier), thumbs);
        assert_eq!(grapheme_floor(text, thumbs), thumbs);
        assert_eq!(grapheme_floor(text, 0), 0);
        assert_eq!(grapheme_floor(text, text.len()), text.len());
        assert_eq!(grapheme_floor("\r\n", 1), 0);
        assert_eq!(grapheme_floor("🇸🇮🇺🇸", 4), 0);
        assert_eq!(grapheme_floor("🇸🇮🇺🇸", 8), 8);
        assert_eq!(grapheme_floor("🇸🇮🇺🇸", 12), 8);
    }

    fn user(id: &str, text: &str) -> GhostEvent {
        GhostEvent::UserMessageDelta {
            message_id: id.into(),
            text: text.into(),
        }
    }

    fn tag(turn: &str) -> TurnTag {
        Some(Arc::from(turn))
    }

    /// Every event of `frames` with its tag, checking that each frame has one tag per event.
    fn tagged_events(frames: &[(EventBatch, Vec<TurnTag>)]) -> Vec<(GhostEvent, TurnTag)> {
        let mut all = Vec::new();
        for (frame, tags) in frames {
            assert_eq!(frame.events.len(), tags.len(), "one tag per event");
            all.extend(frame.events.iter().cloned().zip(tags.iter().cloned()));
        }
        all
    }

    #[test]
    fn user_message_deltas_coalesce_and_split_like_the_others() {
        let mut batcher = FrameBatcher::with_max_bytes("c", 300);
        batcher.push(user("t.u", "Fix "));
        batcher.push(user("t.u", "the build"));
        batcher.push(delta("t.u", " not merged: another kind"));
        assert_eq!(batcher.pending_len(), 2);
        batcher.push(user("t2.u", &"y".repeat(1_000)));
        let frames = batcher.flush();
        let events = all_events(&frames);
        assert_eq!(events[0], user("t.u", "Fix the build"));
        let rebuilt: String = events
            .iter()
            .filter_map(|event| match event {
                GhostEvent::UserMessageDelta { message_id, text } if message_id == "t2.u" => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(rebuilt, "y".repeat(1_000));
        assert!(frames.iter().all(|frame| frame.encoded_len() < 300));
        assert_contiguous(&frames, 0);
    }

    #[test]
    fn only_adjacent_deltas_with_equal_tags_merge() {
        let mut batcher = FrameBatcher::new("c");
        batcher.push_tagged(delta("m", "a"), tag("t1"));
        batcher.push_tagged(delta("m", "b"), tag("t1"));
        batcher.push_tagged(delta("m", "c"), None);
        batcher.push_tagged(delta("m", "d"), None);
        batcher.push_tagged(delta("m", "e"), tag("t2"));
        batcher.push_tagged(thought("th", "f"), tag("t2"));
        let frames = batcher.flush_tagged();
        assert_eq!(
            tagged_events(&frames),
            vec![
                (delta("m", "ab"), tag("t1")),
                (delta("m", "cd"), None),
                (delta("m", "e"), tag("t2")),
                (thought("th", "f"), tag("t2")),
            ]
        );
        assert_eq!(batcher.next_seq(), 4);
    }

    #[test]
    fn split_pieces_keep_their_tag() {
        let mut batcher = FrameBatcher::with_max_bytes("c", 300);
        batcher.push_tagged(notice("before"), None);
        batcher.push_tagged(delta("m", &"x".repeat(2_000)), tag("t"));
        batcher.push_tagged(notice("after"), tag("t"));
        let frames = batcher.flush_tagged();
        assert!(frames.len() > 5, "the delta was split");
        let all = tagged_events(&frames);
        assert_eq!(all.first(), Some(&(notice("before"), None)));
        assert_eq!(all.last(), Some(&(notice("after"), tag("t"))));
        for (event, turn) in &all[1..] {
            assert_eq!(turn, &tag("t"), "{event:?}");
        }
        let plain: Vec<EventBatch> = frames.iter().map(|(frame, _)| frame.clone()).collect();
        assert_eq!(delta_text(&plain, "m").0, "x".repeat(2_000));
        assert_contiguous(&plain, 0);
    }

    #[test]
    fn untagged_flushes_match_tagged_ones() {
        let fill = |batcher: &mut FrameBatcher| {
            batcher.push(notice("one"));
            batcher.push_tagged(delta("m", &"z".repeat(900)), tag("t"));
            batcher.push(user("u", "hi"));
        };
        let mut plain = FrameBatcher::with_max_bytes("c", 400).starting_at(7);
        let mut tagged = plain.clone();
        fill(&mut plain);
        fill(&mut tagged);
        let frames = plain.flush();
        let with_tags: Vec<EventBatch> = tagged
            .flush_tagged()
            .into_iter()
            .map(|(frame, _)| frame)
            .collect();
        assert_eq!(frames, with_tags);
        assert_eq!(plain.next_seq(), tagged.next_seq());
        assert!(plain.flush_tagged().is_empty());
    }

    #[test]
    fn decimal_digits_counts_correctly() {
        for (n, digits) in [(0, 1), (9, 1), (10, 2), (99, 2), (100, 3), (u32::MAX, 10)] {
            assert_eq!(decimal_digits(n), digits);
            assert_eq!(decimal_digits(n), n.to_string().len());
        }
    }
}
