// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! The decode-health counts: what arrived, what was lost, and what each
//! receiver's funnel made of what it heard.

/// What the receiver missed, and what it never had a chance to see.
///
/// This is not a debug panel, it is testimony. Without it
/// every count in the section is a lower bound presented as a total, because the
/// three ways a sample can go missing are all invisible from downstream. The
/// driver drops them before the block is stamped; the bounded feed refuses whole
/// blocks under load and carries no record that it did; and a run broken in the
/// middle takes whatever was being assembled with it.
///
/// Every field here is a count of something that happened, not a rate and not a
/// judgement. The panel does the dividing.
#[derive(Clone, Debug, Default)]
pub struct NetDecodeHealth {
    /// Blocks that reached the worker.
    pub blocks_in: u64,
    /// I/Q pairs in them.
    pub pairs_in: u64,
    /// Times the stream was interrupted: a block that did not continue the one
    /// before it, for either reason.
    ///
    /// **The first block of a run is not one of these.** Opening the section
    /// mid-stream means the first block to arrive is thousands of sequence
    /// numbers past nothing, and counting that as an interruption would put a
    /// phantom gap on the panel every time the user looked at it.
    pub gaps: u64,
    /// A floor on the blocks that went missing, summed over those gaps.
    ///
    /// A floor because the driver says samples went, never how many: one is the
    /// smallest number that is certainly not an overstatement, and zero would
    /// leave the panel calling a lossy link healthy. Same rule as
    /// `BlockPlan::dropped`, which is where this comes from.
    pub blocks_lost: u64,
    /// Unbroken blocks since the last gap.
    pub run_blocks: u64,
    /// Blocks the bounded feed refused in the last poll window, and since the
    /// radio was opened.
    pub refused: u64,
    pub refused_session: u64,
    /// The deepest the feed queue got in the last window.
    pub peak_depth: u64,
    /// When the last block arrived. `None` before the first one.
    pub last_block: Option<std::time::Instant>,
    /// When the feed last lost anything: an interruption, a block the driver
    /// dropped, or a block the bounded feed refused. `None` if it never has.
    ///
    /// The counters above say *how much* went missing; this says *when*, which
    /// is what a panel's feed-loss caveat turns on (`ui::panel::FeedSpan`): a
    /// drop ten minutes ago undercounts a session's census and says nothing
    /// about the dwell that just finished.
    pub last_loss: Option<std::time::Instant>,
    /// How much of real time the worker spends processing the stream: the
    /// wall time it took to handle the last stretch of blocks, over the
    /// stream time those blocks covered. `0.62` is 62 %; above `1.0` the
    /// worker is falling behind and the bounded feed will start refusing
    /// blocks. `None` until a stretch has been measured.
    ///
    /// Foundation design 12.4 makes this a displayed number rather than a
    /// hidden one: every decoder added to the worker spends from it.
    pub decode_load: Option<f64>,
    /// The BLE decode funnel since the section opened
    /// (`signal::ble::receive::Funnel`): triggers, and how each one ended.
    pub ble: crate::signal::ble::receive::Funnel,
    /// LE Coded's decode funnel since the section opened, counted as LE 1M's.
    pub coded: crate::signal::ble::receive::Funnel,
    /// Every AuxPtr followed, by how it ended.
    pub aux: AuxAccounts,
    /// Classic access-code hits since the section opened, every one, where
    /// `bt_hops` keeps only the latest few hundred.
    pub bt_hits: u64,
}

/// Every AuxPtr's promise, by how it ended, since the section opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuxAccounts {
    pub heard: u64,
    pub missed: u64,
    pub not_in_view: u64,
    pub feed_lost: u64,
    pub none_promised: u64,
    pub refused: u64,
}

impl AuxAccounts {
    /// Count one ended promise; a pending one is not ended.
    pub fn count(&mut self, outcome: &crate::signal::ble::aux_ptr::AuxOutcome) {
        use crate::signal::ble::aux_ptr::AuxOutcome;
        match outcome {
            AuxOutcome::Pending => {}
            AuxOutcome::Heard { .. } => self.heard += 1,
            AuxOutcome::Missed => self.missed += 1,
            AuxOutcome::NotInView => self.not_in_view += 1,
            AuxOutcome::FeedLost => self.feed_lost += 1,
            AuxOutcome::NonePromised => self.none_promised += 1,
            AuxOutcome::Refused(_) => self.refused += 1,
        }
    }
}
