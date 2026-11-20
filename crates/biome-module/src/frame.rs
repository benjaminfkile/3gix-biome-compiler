//! The frame this module fills, as the frame record the layer reads
//! describes it (`matter-format.md` section 11).
//!
//! Frame ids are a convention between compiler authors (`matter-format.md`
//! section 5.2): a system module declares the frames, this module builds the
//! overlay of one of them. Which one is the layer config's choice alone: the
//! layer is attached to one frame (`space-model.md` section 8) and reads that
//! frame's record through its one frames slot (`client-compute.md` sections
//! 4.1 and 10.2, `matter-format.md` section 11), for example
//! `"attachment": [4], "inputs": [{"layer": "system", "frames": [4]}]`.
//! The module fills the frame the record names; it holds no frame id of its
//! own. Surface keys of another frame get the empty overlay tile.

use gx_core::frame_record::FrameRecord;

/// The frame the module fills, as its frame record describes it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Frame {
    /// The frame the record describes: the frame the module fills.
    pub frame_id: u64,
    /// Edge of the frame's root cube, meters.
    pub root_extent: f64,
}

/// Why the frame records passed to the module do not describe its frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The number of records is not one: the layer has one frames slot
    /// listing one frame.
    Count(usize),
    /// The record is empty: its layer's module does not declare the frame.
    Empty(u64),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::Count(n) => write!(
                f,
                "{n} frame records passed; the module reads exactly one, the record of the frame it fills"
            ),
            FrameError::Empty(id) => write!(
                f,
                "the record of frame {id} is empty: the system layer does not declare the frame, so its root extent is unknown"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    /// The frame of a record: the frame it names, which must be declared
    /// (a non-empty record).
    pub fn from_record(record: &FrameRecord) -> Result<Frame, FrameError> {
        if record.empty {
            return Err(FrameError::Empty(record.frame_id));
        }
        Ok(Frame {
            frame_id: record.frame_id,
            root_extent: record.root_extent.value(),
        })
    }

    /// The frame of the records passed to the module: exactly one non-empty
    /// record (see [`Frame::from_record`]).
    pub fn from_records(records: &[FrameRecord]) -> Result<Frame, FrameError> {
        match records {
            [r] => Frame::from_record(r),
            _ => Err(FrameError::Count(records.len())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gx_core::units::Meters;

    fn record(frame_id: u64, root_extent: f64) -> FrameRecord {
        FrameRecord {
            root_extent: Meters::new(root_extent),
            empty: false,
            max_depth: 6,
            ..FrameRecord::empty(frame_id)
        }
    }

    #[test]
    fn the_record_names_the_frame() {
        for id in [3, 4, 12] {
            assert_eq!(
                Frame::from_record(&record(id, 1e7)),
                Ok(Frame {
                    frame_id: id,
                    root_extent: 1e7
                })
            );
        }
    }

    #[test]
    fn records_that_do_not_describe_a_frame_fail() {
        assert_eq!(
            Frame::from_record(&FrameRecord::empty(4)),
            Err(FrameError::Empty(4))
        );
        assert_eq!(Frame::from_records(&[]), Err(FrameError::Count(0)));
        let r = record(4, 1e7);
        assert_eq!(Frame::from_records(&[r, r]), Err(FrameError::Count(2)));
        assert_eq!(
            Frame::from_records(&[r]),
            Ok(Frame {
                frame_id: 4,
                root_extent: 1e7
            })
        );
    }
}
