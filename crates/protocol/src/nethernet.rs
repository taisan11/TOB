//! NetherNet data-channel framing used by recent Bedrock Dedicated Servers.
//!
//! This module handles the message-level fragment counter documented for the
//! ReliableDataChannel and UnreliableDataChannel. ICE, DTLS, SCTP, signaling,
//! and the Bedrock game-packet payload are separate layers.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataChannel {
    Reliable,
    Unreliable,
}

impl DataChannel {
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "ReliableDataChannel" => Some(Self::Reliable),
            "UnreliableDataChannel" => Some(Self::Unreliable),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    Empty,
    UnexpectedFragment,
    MissingFragment,
    OutOfOrder { expected: u8, got: u8 },
    TooManyFragments,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty NetherNet data-channel frame"),
            Self::UnexpectedFragment => f.write_str("fragment received on unreliable channel"),
            Self::MissingFragment => {
                f.write_str("unfragmented frame interrupted a fragmented message")
            }
            Self::OutOfOrder { expected, got } => {
                write!(f, "out-of-order fragment: expected {expected}, got {got}")
            }
            Self::TooManyFragments => f.write_str("NetherNet fragment counter overflow"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Reassembles NetherNet's one-byte countdown-framed SCTP messages.
///
/// On the reliable ordered channel, a nonzero prefix starts/continues a fragmented
/// message and zero terminates it. Each subsequent prefix must count down by one.
/// The unreliable channel only carries complete messages (prefix zero).
#[derive(Default)]
pub struct Reassembler {
    expected_remaining: Option<u8>,
    buffer: Vec<u8>,
}

impl Reassembler {
    /// Consumes one SCTP message and returns a complete application packet when ready.
    pub fn push(
        &mut self,
        channel: DataChannel,
        frame: &[u8],
    ) -> Result<Option<Vec<u8>>, FrameError> {
        let (&remaining, payload) = frame.split_first().ok_or(FrameError::Empty)?;
        if channel == DataChannel::Unreliable && remaining != 0 {
            self.reset();
            return Err(FrameError::UnexpectedFragment);
        }

        match self.expected_remaining {
            None if remaining == 0 => Ok(Some(payload.to_vec())),
            None => {
                self.buffer.extend_from_slice(payload);
                self.expected_remaining = Some(remaining - 1);
                Ok(None)
            }
            Some(expected) if remaining == expected => {
                self.buffer.extend_from_slice(payload);
                if remaining == 0 {
                    self.expected_remaining = None;
                    Ok(Some(std::mem::take(&mut self.buffer)))
                } else {
                    self.expected_remaining = Some(remaining - 1);
                    Ok(None)
                }
            }
            Some(expected) => {
                self.reset();
                Err(FrameError::OutOfOrder {
                    expected,
                    got: remaining,
                })
            }
        }
    }

    /// Splits one message into countdown-framed SCTP messages of at most `max_payload`
    /// bytes each. The caller must use the reliable data channel for multi-frame output.
    pub fn fragment(payload: &[u8], max_payload: usize) -> Result<Vec<Vec<u8>>, FrameError> {
        if max_payload == 0 {
            return Err(FrameError::TooManyFragments);
        }
        let count = payload.len().max(1).div_ceil(max_payload);
        if count > usize::from(u8::MAX) + 1 {
            return Err(FrameError::TooManyFragments);
        }
        let mut result = Vec::with_capacity(count);
        for (index, part) in payload.chunks(max_payload).enumerate() {
            let remaining =
                u8::try_from(count - index - 1).map_err(|_| FrameError::TooManyFragments)?;
            let mut frame = Vec::with_capacity(part.len() + 1);
            frame.push(remaining);
            frame.extend_from_slice(part);
            result.push(frame);
        }
        if payload.is_empty() {
            result.push(vec![0]);
        }
        Ok(result)
    }

    pub fn reset(&mut self) {
        self.expected_remaining = None;
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_frames_round_trip_on_both_channels() {
        for channel in [DataChannel::Reliable, DataChannel::Unreliable] {
            let mut reassembler = Reassembler::default();
            assert_eq!(
                reassembler.push(channel, b"\0hello").unwrap(),
                Some(b"hello".to_vec())
            );
        }
    }

    #[test]
    fn reliable_fragments_reassemble_in_countdown_order() {
        let frames = Reassembler::fragment(b"abcdefg", 3).unwrap();
        assert_eq!(
            frames,
            vec![b"\x02abc".to_vec(), b"\x01def".to_vec(), b"\0g".to_vec()]
        );
        let mut reassembler = Reassembler::default();
        assert_eq!(
            reassembler.push(DataChannel::Reliable, &frames[0]).unwrap(),
            None
        );
        assert_eq!(
            reassembler.push(DataChannel::Reliable, &frames[1]).unwrap(),
            None
        );
        assert_eq!(
            reassembler.push(DataChannel::Reliable, &frames[2]).unwrap(),
            Some(b"abcdefg".to_vec())
        );
    }

    #[test]
    fn unreliable_fragmentation_is_rejected() {
        let mut reassembler = Reassembler::default();
        assert_eq!(
            reassembler.push(DataChannel::Unreliable, b"\x01x"),
            Err(FrameError::UnexpectedFragment)
        );
    }

    #[test]
    fn wrong_countdown_resets_partial_packet() {
        let mut reassembler = Reassembler::default();
        assert_eq!(
            reassembler.push(DataChannel::Reliable, b"\x02a").unwrap(),
            None
        );
        assert_eq!(
            reassembler.push(DataChannel::Reliable, b"\0b"),
            Err(FrameError::OutOfOrder {
                expected: 1,
                got: 0
            })
        );
        assert_eq!(
            reassembler.push(DataChannel::Reliable, b"\0c").unwrap(),
            Some(b"c".to_vec())
        );
    }
}
