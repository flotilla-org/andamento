//! Timestamped connector JSONL shared by tests and native replay tools.
//! Offsets are monotonic milliseconds from capture start; equal offsets retain
//! file order. Legacy bare patches are accepted at offset zero.
use crate::{MetadataPatch, Sidebar};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub offset_ms: u64,
    #[serde(serialize_with = "serialize_patch")]
    pub patch: MetadataPatch,
}

/// Connector envelope, including the discriminator expected by transport readers.
pub fn wire_patch(patch: &MetadataPatch) -> serde_json::Value {
    let mut value = serde_json::to_value(patch).expect("metadata patch is serializable");
    value["type"] = serde_json::Value::String("metadata-patch".into());
    value
}

fn serialize_patch<S: serde::Serializer>(
    patch: &MetadataPatch,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    wire_patch(patch).serialize(serializer)
}

pub fn read(reader: impl BufRead) -> io::Result<Vec<Frame>> {
    let mut frames = Vec::new();
    let mut previous = 0;
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed = (|| -> Result<Frame, serde_json::Error> {
            let value: serde_json::Value = serde_json::from_str(&line)?;
            if value.get("offset_ms").is_some() || value.get("patch").is_some() {
                serde_json::from_value(value)
            } else {
                Ok(Frame {
                    offset_ms: 0,
                    patch: serde_json::from_value(value)?,
                })
            }
        })();
        let frame = parsed.map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {e}", index + 1),
            )
        })?;
        if frame.offset_ms < previous {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: decreasing offset", index + 1),
            ));
        }
        previous = frame.offset_ms;
        frames.push(frame);
    }
    Ok(frames)
}

/// Applies all patches through a requested instant, preserving arrival time for
/// TTL calculation. Advancing beyond the last patch also expires stale facts.
pub struct Replay {
    frames: std::vec::IntoIter<Frame>,
    now_ms: u64,
}
impl Replay {
    pub fn new(frames: Vec<Frame>) -> io::Result<Self> {
        if frames
            .windows(2)
            .any(|pair| pair[0].offset_ms > pair[1].offset_ms)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "decreasing offset",
            ));
        }
        Ok(Self {
            frames: frames.into_iter(),
            now_ms: 0,
        })
    }
    pub fn next_offset(&self) -> Option<u64> {
        self.frames.as_slice().first().map(|f| f.offset_ms)
    }
    pub fn advance_to(&mut self, sidebar: &mut Sidebar, offset_ms: u64) -> io::Result<()> {
        if offset_ms < self.now_ms {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot rewind replay",
            ));
        }
        while self.next_offset().is_some_and(|at| at <= offset_ms) {
            let frame = self.frames.next().unwrap();
            sidebar.apply(frame.offset_ms, [frame.patch]);
        }
        sidebar.apply(offset_ms, []);
        self.now_ms = offset_ms;
        Ok(())
    }
    /// Step one timestamp (all simultaneous patches form one observation).
    pub fn step(&mut self, sidebar: &mut Sidebar) -> io::Result<Option<u64>> {
        let Some(at) = self.next_offset() else {
            return Ok(None);
        };
        self.advance_to(sidebar, at)?;
        Ok(Some(at))
    }
}

/// A process-local monotonic tap. Flush each frame so interrupted captures
/// preserve every complete line. Call at the outgoing connector boundary.
pub struct Recorder<W> {
    writer: W,
    start: Instant,
}
impl<W: Write> Recorder<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            start: Instant::now(),
        }
    }
    pub fn record(&mut self, patch: MetadataPatch) -> io::Result<()> {
        let offset_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        serde_json::to_writer(&mut self.writer, &Frame { offset_ms, patch })?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
}
