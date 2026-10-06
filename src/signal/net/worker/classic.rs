// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Classic Bluetooth's receivers: how many the survey can afford, and the
//! fleet run side by side with the BLE receiver on one block.

use crate::signal::ble::receive::Receiver as BleReceiver;
use crate::signal::bt::receive::Receiver as BtReceiver;

/// The survey watches one classic channel fewer once its measured load
/// passes this, and one more once it falls under [`SURVEY_LOAD_LOW`], up to
/// the Classic view's own cap. The survey is there to measure the band: a
/// classic receiver that pushed it past real time would cost it blocks, and
/// every duty cycle on screen with them. Measured on the i3 before this was
/// built: the survey alone 0.61x at 8 Msps and 1.00x at 20, a classic
/// channel about 0.18x and 0.28x more, so room for about one channel at 8
/// and none at 20; a faster machine gets more.
const SURVEY_LOAD_HIGH: f64 = 0.8;
const SURVEY_LOAD_LOW: f64 = 0.7;

/// The survey's classic channel count after a load `reading`: one fewer
/// over [`SURVEY_LOAD_HIGH`], one more under [`SURVEY_LOAD_LOW`] up to `cap`,
/// unchanged between them, so it settles rather than hunting.
pub(super) fn survey_budget(now: usize, reading: f64, cap: usize) -> usize {
    if reading > SURVEY_LOAD_HIGH {
        now.saturating_sub(1)
    } else if reading < SURVEY_LOAD_LOW {
        (now + 1).min(cap)
    } else {
        now
    }
}

/// What one classic receiver heard in a block: its hits and its headers.
pub(super) type Heard = (
    Vec<crate::signal::bt::receive::AccessHit>,
    Vec<crate::signal::bt::receive::HeaderHit>,
);

/// Every classic receiver of the fleet fed `iq`, their answers in fleet
/// order, the receivers spread over the machine's cores.
///
/// **Parallel because nothing is shared, and so exact.** Each receiver owns
/// its mixer, filter, lanes and captures, and reads the same block; run on
/// several threads and put back in order, they give what one thread gives,
/// bit for bit. On the i3 the Classic view at 4 Msps was 1.4 times real time
/// on one core, most of it three receivers doing the same work side by side.
/// A block is milliseconds long, so starting the threads for each is lost in
/// it.
pub(super) fn push_fleet(fleet: &mut [BtReceiver], iq: &[num_complex::Complex<f32>]) -> Vec<Heard> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = cores.min(fleet.len());
    if threads <= 1 {
        return fleet.iter_mut().map(|rx| rx.push_iq(iq)).collect();
    }
    let per = fleet.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let running: Vec<_> = fleet
            .chunks_mut(per)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter_mut()
                        .map(|rx| rx.push_iq(iq))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        running
            .into_iter()
            .flat_map(|t| {
                t.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// The BLE receiver, when there is one to feed, and the classic fleet, fed
/// the same block at once: the BLE receiver on a thread of its own beside
/// [`push_fleet`]'s. Each owns everything it touches, so running them side
/// by side gives what running them one after the other gave, and the BLE
/// receiver's packets are handled after the fleet's hits are in, both in
/// the order they always were.
pub(super) fn push_all(
    ble: Option<&mut BleReceiver>,
    fleet: &mut [BtReceiver],
    iq: &[num_complex::Complex<f32>],
    first_pair: u64,
) -> (Option<Vec<crate::signal::ble::pdu::Packet>>, Vec<Heard>) {
    let Some(rx) = ble else {
        return (None, push_fleet(fleet, iq));
    };
    // Nothing to run beside it: a thread started and joined for one decoder
    // is a cost with no second decoder to pay for it.
    if fleet.is_empty() {
        return (Some(rx.push_iq_at(iq, first_pair)), Vec::new());
    }
    std::thread::scope(|scope| {
        let packets = scope.spawn(move || rx.push_iq_at(iq, first_pair));
        let answers = push_fleet(fleet, iq);
        let packets = packets
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (Some(packets), answers)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The survey's classic budget moves one channel at a time, down over
    /// the high mark, up under the low one to the cap, and holds between.
    #[test]
    fn the_survey_budget_follows_the_load_it_measured() {
        assert_eq!(survey_budget(0, 0.5, 8), 1);
        assert_eq!(survey_budget(8, 0.5, 8), 8, "capped");
        assert_eq!(survey_budget(3, 0.95, 8), 2);
        assert_eq!(survey_budget(0, 0.95, 8), 0);
        assert_eq!(survey_budget(3, 0.75, 8), 3, "held between the marks");
    }
}
