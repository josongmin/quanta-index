//! WAL subscriber. Tails segment files in order, persisting a `cursor` file on
//! `ack`. Subscriber polls non-blocking — backend consumers wrap this in their
//! own poll loop (`searchd::app::dispatcher`).

use std::path::PathBuf;

use quanta_index_contract::channel::ChannelSeq;

use crate::api::error::ChannelError;
use crate::api::subscriber::BundleChannelSubscriber;

use super::codec::OpCodec;
use super::cursor::CursorFile;
use super::segment::{SegmentLayout, SegmentReader};

pub struct WalSubscriber<C: OpCodec> {
    layout: SegmentLayout,
    cursor: CursorFile,
    reader: Option<SegmentReader>,
    last_emitted: ChannelSeq,
    _codec: std::marker::PhantomData<fn() -> C>,
}

impl<C: OpCodec> WalSubscriber<C> {
    pub fn open_with_codec(track_root: PathBuf, _codec: C) -> Result<Self, ChannelError> {
        let layout = SegmentLayout::new(track_root);
        let cursor = CursorFile::open(layout.cursor_path())?;
        let starting = cursor.value();
        let mut sub = Self {
            layout,
            cursor,
            reader: None,
            last_emitted: starting,
            _codec: std::marker::PhantomData,
        };
        sub.open_initial_segment()?;
        Ok(sub)
    }

    /// Open the lowest segment with `seg_id` >= 0. `next_event` will skip
    /// frames with seq <= cursor naturally; this avoids needing a per-frame
    /// index.
    fn open_initial_segment(&mut self) -> Result<(), ChannelError> {
        let segments = self.layout.list_segments()?;
        let Some((seg_id, _)) = segments.first() else {
            return Ok(());
        };
        let reader = SegmentReader::open(&self.layout, *seg_id)?;
        self.reader = Some(reader);
        Ok(())
    }

    fn open_next_segment(&mut self, after: u64) -> Result<bool, ChannelError> {
        let segments = self.layout.list_segments()?;
        for (seg_id, _) in segments {
            if seg_id > after {
                let reader = SegmentReader::open(&self.layout, seg_id)?;
                self.reader = Some(reader);
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<C: OpCodec> BundleChannelSubscriber for WalSubscriber<C> {
    type Event = C::Event;

    fn next_event(&mut self) -> Result<Option<Self::Event>, ChannelError> {
        loop {
            let Some(reader) = self.reader.as_mut() else {
                // No segments yet — try to open one now (producer may have
                // written its first segment after our open).
                self.open_initial_segment()?;
                if self.reader.is_none() {
                    return Ok(None);
                }
                continue;
            };
            reader.refresh_len()?;
            match reader.read_next_frame() {
                Ok(Some((seq, body))) => {
                    if seq.get() <= self.cursor.value().get() {
                        // already acknowledged — silently skip
                        continue;
                    }
                    if seq.get() <= self.last_emitted.get() {
                        return Err(ChannelError::Corrupted {
                            at_seq: seq,
                            reason: "seq regression vs last emitted".to_string(),
                        });
                    }
                    let op = C::decode_op(&body)?;
                    self.last_emitted = seq;
                    return Ok(Some(C::event(seq, op)));
                }
                Ok(None) => {
                    let seg_id = reader.seg_id();
                    if !self.open_next_segment(seg_id)? {
                        return Ok(None);
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn ack(&mut self, up_to: ChannelSeq) -> Result<(), ChannelError> {
        if up_to.get() < self.cursor.value().get() {
            return Err(ChannelError::State(format!(
                "cursor regression rejected (have {}, ack {})",
                self.cursor.value().get(),
                up_to.get()
            )));
        }
        self.cursor.store(up_to)?;
        Ok(())
    }

    fn cursor(&self) -> ChannelSeq {
        self.cursor.value()
    }
}
