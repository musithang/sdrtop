// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Continuity accounting for a [`crate::hardware::StreamBlock`] feed.
//!
//! Every consumer of one of those channels asks the same two questions of every
//! block - does this one continue the last, and how many never arrived - and
//! neither question has anything to do with what the consumer is decoding. It
//! lived in the demod worker while the demod worker was the only consumer, and
//! moved here when the NET worker became the second.
//!
//! It has to be shared rather than reimplemented because getting it wrong is not
//! a crash. It is a run read as unbroken straight across a hole, and a
//! measurement made over the join that looks entirely plausible.

/// What the worker knows about a block before it looks at the samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockPlan {
    /// This block follows the previous one with nothing missing between them.
    pub contiguous: bool,
    /// How many blocks the bounded channel lost since the last one that reached
    /// us. `process_block` forwards with `try_send` and discards the
    /// result, so this gap is the only record that they existed.
    pub dropped: u64,
}

/// Read the sequence numbers, and what the driver said about the block itself.
///
/// Two sources, because there are two ways to lose audio and neither can see the
/// other. `seq` catches a block lost after it was stamped; `gap_before` catches
/// samples lost before it ever was. Either one means this block does not
/// continue the last, and the detectors that span blocks have to start again.
///
/// `drop_ref` is the sequence of the previous block *that was forwarded to us*,
/// or `None` when the run has not started - distinct from `last_seq` because the
/// device counts every callback, so the jump across a stretch when the demod was
/// switched off is not a loss and must not be counted as one.
///
/// Wrapping throughout: the counter is a `u64` that never resets, and arithmetic
/// that panics on overflow in a release build would silently do the wrong thing
/// in a debug one.
pub fn plan_block(seq: u64, gap_before: bool, last_seq: u64, drop_ref: Option<u64>) -> BlockPlan {
    let lost_after = drop_ref.map_or(0, |prev| seq.wrapping_sub(prev).saturating_sub(1));
    BlockPlan {
        contiguous: !gap_before && seq == last_seq.wrapping_add(1),
        // A floor, the same shape as the one `soapy::stream` uses for an
        // overflow: the driver says samples went, never how many. One is the
        // smallest number that is certainly not an overstatement, and zero would
        // leave the panel calling a lossy link healthy.
        dropped: lost_after + u64::from(gap_before),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_that_follows_the_last_one_is_contiguous() {
        assert!(plan_block(5, false, 4, Some(4)).contiguous);
        assert!(!plan_block(6, false, 4, Some(4)).contiguous);
        // The very first block: nothing precedes it, so nothing was dropped.
        assert_eq!(plan_block(0, false, 0, None).dropped, 0);
    }

    /// Samples the driver lost break the run, though the numbers are consecutive.
    ///
    /// The sequence counter is stamped inside `process_block`, so it can only
    /// see a block lost *after* that point. A HackRF short transfer or a
    /// SoapySDR overflow throws samples away *before* it, and the block that
    /// follows still gets the next number - so the run read as unbroken across a
    /// real hole in the audio. CTCSS went on filling its half-second window and
    /// reported a tone measured across the join; RDS carried its bit phase over
    /// the gap. Both said "this station decodes" when what had happened was that
    /// the radio lost samples.
    #[test]
    fn samples_lost_by_the_driver_break_the_run() {
        // As far as the channel is concerned, 5 follows 4 with nothing missing.
        assert!(plan_block(5, false, 4, Some(4)).contiguous);
        // The driver says otherwise, and the driver is the one that would know.
        let p = plan_block(5, true, 4, Some(4));
        assert!(
            !p.contiguous,
            "the audio in this block does not continue the audio in the last one"
        );
        assert_eq!(
            p.dropped, 1,
            "a floor: the driver says samples went, not how many blocks' worth"
        );
    }

    /// The two kinds of loss add up rather than masking each other.
    #[test]
    fn a_driver_loss_and_a_channel_loss_are_both_counted() {
        // Blocks 5 and 6 were lost by the channel, and the driver lost samples
        // before block 7 as well.
        assert_eq!(plan_block(7, true, 4, Some(4)).dropped, 3);
    }

    #[test]
    fn dropped_counts_the_gap_and_not_the_block_itself() {
        // 4 then 7: blocks 5 and 6 were lost, which is two, not three.
        assert_eq!(plan_block(7, false, 4, Some(4)).dropped, 2);
        assert_eq!(
            plan_block(5, false, 4, Some(4)).dropped,
            0,
            "back to back loses nothing"
        );
    }

    #[test]
    fn a_stretch_with_the_demod_off_is_not_a_drop() {
        // `drop_ref` is cleared when the demod is switched off, so the jump back
        // in is not counted. Without that, every pause would report thousands of
        // lost blocks and the panel would blame the host.
        assert_eq!(plan_block(9_000, false, 8_999, None).dropped, 0);
    }

    #[test]
    fn the_sequence_counter_wrapping_does_not_invent_a_drop() {
        // The counter never resets, so the arithmetic has to wrap rather than
        // panic in debug or produce a vast bogus gap in release.
        let p = plan_block(0, false, u64::MAX, Some(u64::MAX));
        assert!(p.contiguous, "0 follows u64::MAX");
        assert_eq!(p.dropped, 0);
        assert_eq!(plan_block(2, false, u64::MAX, Some(u64::MAX)).dropped, 2);
    }
}
