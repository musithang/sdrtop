// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! Rate-1/2 convolutional codes and their Viterbi decoder, for any
//! constraint length up to 7 and any two generators: Bluetooth LE Coded's
//! (constraint length 4) and 802.11's (7) are the same algorithm with other
//! numbers.
//!
//! **One convention for the generators**, the one 802.11 writes them in
//! (IEEE 802.11-2024, 17.3.5.6): read most significant bit first, a
//! generator taps the current input and then each earlier input in turn. LE
//! Coded's `G0(x) = 1 + x + x^2 + x^3` is therefore `0o17` and
//! `G1(x) = 1 + x^2 + x^3` is `0o13` (Core v5.4, Vol 6, Part B, 3.3.1).
//!
//! **The decoder chooses as LE Coded's did, to the bit.** Where two paths
//! into a state cost the same, the one from the lower-numbered state wins;
//! an unterminated stream ends at the cheapest state, the lowest of equals.
//! The test module keeps that decoder, as it was, to hold this one to it.
//!
//! The work a step is the same either way; what is new is how it is held:
//! a step's four branch costs are priced once, not once a branch, and each
//! state's survivor is the one bit that tells its two predecessors apart,
//! packed 64 states to a word.

/// A rate-1/2 code: constraint length and its two generators, output A
/// (from `g0`) before B.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code {
    /// Bits of memory, the constraint length less one: 3 for LE Coded, 6 for
    /// 802.11.
    memory: u32,
    g0: u32,
    g1: u32,
}

impl Code {
    /// A code of `constraint` length (2 to 7) and generators `g0`, `g1` in
    /// the convention above.
    pub const fn new(constraint: u32, g0: u32, g1: u32) -> Self {
        assert!(
            constraint >= 2 && constraint <= 7,
            "constraint length 2 to 7"
        );
        Self {
            memory: constraint - 1,
            g0,
            g1,
        }
    }

    fn states(self) -> usize {
        1 << self.memory
    }

    /// Outputs A and B for a window: the current input in bit `memory`, the
    /// state (the inputs before it, the latest highest) below.
    fn outputs(self, window: u32) -> (bool, bool) {
        (
            (window & self.g0).count_ones() % 2 == 1,
            (window & self.g1).count_ones() % 2 == 1,
        )
    }

    /// Encode from the zero state, two coded bits for every one in, A then
    /// B. Termination is the caller's: so many zeros appended as the code
    /// has memory bring it back to the zero state.
    pub fn encode(self, bits: &[bool]) -> Vec<bool> {
        let mut state = 0u32;
        let mut out = Vec::with_capacity(bits.len() * 2);
        for &b in bits {
            let window = (b as u32) << self.memory | state;
            let (a, bb) = self.outputs(window);
            out.push(a);
            out.push(bb);
            state = window >> 1;
        }
        out
    }

    /// The maximum-likelihood input over `steps` input bits, `cost` pricing
    /// a branch at step `k` that would have sent A and B: lower is likelier.
    /// `terminated` ends the path at the zero state; otherwise at the
    /// cheapest. `None` for no steps, or a cost that leaves no state
    /// reachable.
    pub fn decode(
        self,
        steps: usize,
        terminated: bool,
        cost: impl Fn(usize, bool, bool) -> f32,
    ) -> Option<Vec<bool>> {
        if steps == 0 {
            return None;
        }
        let n = self.states();
        let top = self.memory - 1;
        // For each state, the outputs its two predecessors would have sent
        // to reach it, as indices into a step's four prices (A in bit 1, B
        // in bit 0): the lower-numbered predecessor first.
        let mut sends = [[0usize; 2]; 64];
        for (to, pair) in sends.iter_mut().enumerate().take(n) {
            let input = (to as u32 >> top) & 1;
            let base = ((to << 1) & (n - 1)) as u32;
            for (dropped, s) in pair.iter_mut().enumerate() {
                let (a, b) = self.outputs(input << self.memory | base | dropped as u32);
                *s = (a as usize) << 1 | b as usize;
            }
        }
        let mut metric = [f32::INFINITY; 64];
        metric[0] = 0.0;
        let mut next = [f32::INFINITY; 64];
        let mut survivors: Vec<u64> = Vec::with_capacity(steps);

        for k in 0..steps {
            let price = [
                cost(k, false, false),
                cost(k, false, true),
                cost(k, true, false),
                cost(k, true, true),
            ];
            let mut chosen = 0u64;
            for to in 0..n {
                let base = (to << 1) & (n - 1);
                // Each predecessor's path plus its branch. An unreached one
                // stays infinite and never wins; of two equal, the lower-
                // numbered keeps it, by the strict comparison.
                let via0 = metric[base] + price[sends[to][0]];
                let via1 = metric[base | 1] + price[sends[to][1]];
                let take1 = via1 < via0;
                next[to] = if take1 { via1 } else { via0 };
                chosen |= (take1 as u64) << to;
            }
            std::mem::swap(&mut metric, &mut next);
            survivors.push(chosen);
        }

        let end = if terminated {
            0
        } else {
            (0..n).min_by(|&a, &b| metric[a].total_cmp(&metric[b]))?
        };
        if metric[end].is_infinite() {
            return None;
        }
        let mut bits = vec![false; steps];
        let mut state = end;
        for k in (0..steps).rev() {
            bits[k] = (state >> top) & 1 == 1;
            let dropped = (survivors[k] >> state) & 1;
            state = ((state << 1) & (n - 1)) | dropped as usize;
        }
        Some(bits)
    }
}

/// A branch's hard cost at step `k`: how many of the two received bits it
/// disagrees with. Small whole numbers, exact in an `f32`.
pub fn hamming(coded: &[bool], k: usize, a: bool, b: bool) -> f32 {
    f32::from(u8::from(a != coded[2 * k]) + u8::from(b != coded[2 * k + 1]))
}

/// A branch's soft cost at step `k`: minus the readings (positive for a 1)
/// it expects to be positive, plus the ones it expects negative. A reading
/// of zero, an erasure, favours neither.
pub fn correlation(readings: &[f32], k: usize, a: bool, b: bool) -> f32 {
    let one = |expected: bool, v: f32| if expected { -v } else { v };
    one(a, readings[2 * k]) + one(b, readings[2 * k + 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::dsp::testkit::Rng;

    const LE_CODED: Code = Code::new(4, 0o17, 0o13);
    const WIFI: Code = Code::new(7, 0o133, 0o171);

    fn bits(s: &str) -> Vec<bool> {
        s.chars().map(|c| c == '1').collect()
    }

    /// A one followed by zeros rings out each generator, A and B, current
    /// input first: LE Coded's 1111 and 1011, 802.11's 1011011 and 1111001.
    #[test]
    fn an_impulse_rings_out_the_generators() {
        for (code, len, a, b) in [
            (LE_CODED, 4, "1111", "1011"),
            (WIFI, 7, "1011011", "1111001"),
        ] {
            let mut input = vec![false; len];
            input[0] = true;
            let out = code.encode(&input);
            let got_a: Vec<bool> = out.iter().step_by(2).copied().collect();
            let got_b: Vec<bool> = out.iter().skip(1).step_by(2).copied().collect();
            assert_eq!((got_a, got_b), (bits(a), bits(b)), "{code:?}");
        }
    }

    /// A hand-checked constraint-length-7 sequence: 1101 then six zeros of
    /// termination, encoded by superposing the impulse response shifted to
    /// each one; decoded back with two coded bits flipped, far apart.
    #[test]
    fn a_short_constraint_length_seven_sequence_decodes_through_errors() {
        let input = bits("1101000000");
        let coded = WIFI.encode(&input);
        // Each output is the XOR of the impulse responses at the ones (0, 1
        // and 3), A from 1011011 and B from 1111001.
        let ring = |g: &str, at: usize| -> Vec<bool> {
            (0..10)
                .map(|i| i >= at && i - at < 7 && g.as_bytes()[i - at] == b'1')
                .collect()
        };
        for (out, g) in [(0, "1011011"), (1, "1111001")] {
            let want: Vec<bool> = (0..10)
                .map(|i| ring(g, 0)[i] ^ ring(g, 1)[i] ^ ring(g, 3)[i])
                .collect();
            let got: Vec<bool> = coded.iter().skip(out).step_by(2).copied().collect();
            assert_eq!(got, want, "output {out}");
        }
        let mut hit = coded.clone();
        hit[2] = !hit[2];
        hit[15] = !hit[15];
        let decoded = WIFI.decode(10, true, |k, a, b| hamming(&hit, k, a, b));
        assert_eq!(decoded, Some(input));
    }

    /// Where every path costs the same, the decoder keeps the zero path:
    /// each tie goes to the lower-numbered predecessor and an unterminated
    /// end to the lowest state.
    #[test]
    fn of_equal_paths_the_earliest_wins() {
        for code in [LE_CODED, WIFI] {
            let zero = vec![0.0f32; 40];
            let got = code.decode(20, false, |k, a, b| correlation(&zero, k, a, b));
            assert_eq!(got, Some(vec![false; 20]), "{code:?}");
        }
    }

    /// No steps, nothing to decode.
    #[test]
    fn no_steps_is_no_answer() {
        assert_eq!(WIFI.decode(0, true, |_, _, _| 0.0), None);
    }

    /// **Bit for bit what LE Coded's own decoder chose**, kept below as it
    /// was: hard and soft, terminated and not, over thousands of noisy
    /// blocks, at noise from barely any to more than the code can carry.
    #[test]
    fn le_coded_decodes_exactly_as_its_own_decoder_did() {
        let mut rng = Rng::new(29);
        for round in 0..3000 {
            let steps = 4 + round % 60;
            let input: Vec<bool> = (0..steps).map(|i| (i * 13 + round) % 7 < 3).collect();
            let clean: Vec<f32> = LE_CODED
                .encode(&input)
                .iter()
                .map(|&b| if b { 1.0 } else { -1.0 })
                .collect();
            let power = [0.05, 0.5, 1.5, 4.0][round % 4];
            let noise = rng.noise(clean.len(), power);
            let soft: Vec<f32> = clean.iter().zip(&noise).map(|(c, n)| c + n.re).collect();
            let hard: Vec<bool> = soft.iter().map(|&v| v > 0.0).collect();
            for terminated in [true, false] {
                assert_eq!(
                    LE_CODED.decode(steps, terminated, |k, a, b| correlation(&soft, k, a, b)),
                    before::viterbi(soft.len(), terminated, |k, a, b| correlation(
                        &soft, k, a, b
                    )),
                    "soft, round {round}"
                );
                assert_eq!(
                    LE_CODED.decode(steps, terminated, |k, a, b| hamming(&hard, k, a, b)),
                    before::viterbi(hard.len(), terminated, |k, a, b| hamming(&hard, k, a, b)),
                    "hard, round {round}"
                );
            }
            assert_eq!(LE_CODED.encode(&input), before::encode(&input));
        }
    }

    /// **What it costs beside what it replaced**, in one run on one machine.
    /// Not a check: by hand, in release (`cargo test --release
    /// decoder_cost_against -- --ignored --nocapture`). LE Coded's FEC
    /// block 2 for a 37-octet advertisement, soft, noisy, decoded by the
    /// decoder kept below and by this one, in turns, so a busy machine
    /// slows both alike.
    #[test]
    #[ignore]
    fn decoder_cost_against_the_one_it_replaced() {
        let mut input: Vec<bool> = (0..(2 + 37) * 8 + 24).map(|i| i * 7 % 5 < 2).collect();
        input.extend([false; 3]);
        let clean: Vec<f32> = LE_CODED
            .encode(&input)
            .iter()
            .map(|&b| if b { 1.0 } else { -1.0 })
            .collect();
        let noise = Rng::new(3).noise(clean.len(), 0.4);
        let r: Vec<f32> = clean.iter().zip(&noise).map(|(c, n)| c + n.re).collect();
        let (mut old, mut new) = (0.0f64, 0.0f64);
        const ROUNDS: usize = 5_000;
        for _ in 0..4 {
            let t = std::time::Instant::now();
            for _ in 0..ROUNDS {
                std::hint::black_box(before::viterbi(r.len(), true, |k, a, b| {
                    correlation(std::hint::black_box(&r), k, a, b)
                }));
            }
            old += t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            for _ in 0..ROUNDS {
                std::hint::black_box(LE_CODED.decode(r.len() / 2, true, |k, a, b| {
                    correlation(std::hint::black_box(&r), k, a, b)
                }));
            }
            new += t.elapsed().as_secs_f64();
        }
        let per = |t: f64| t / (4 * ROUNDS) as f64 * 1e6;
        eprintln!(
            "LE Coded block 2, {} bits: before {:.1} us, now {:.1} us ({:.0} % of before)",
            input.len(),
            per(old),
            per(new),
            new / old * 100.0
        );
    }

    /// LE Coded's encoder and decoder as they stood before this module, word
    /// for word, kept as the reference the shared code is held to.
    mod before {
        type State = [bool; 3];
        const STATE_COUNT: usize = 8;

        fn state_to_index(s: State) -> usize {
            ((s[0] as usize) << 2) | ((s[1] as usize) << 1) | (s[2] as usize)
        }

        fn index_to_state(i: usize) -> State {
            [(i >> 2) & 1 != 0, (i >> 1) & 1 != 0, i & 1 != 0]
        }

        fn step(state: State, input: bool) -> (bool, bool, State) {
            let taps = [input, state[0], state[1], state[2]];
            let g0 = taps[0] ^ taps[1] ^ taps[2] ^ taps[3];
            let g1 = taps[0] ^ taps[2] ^ taps[3];
            let next = [input, state[0], state[1]];
            (g0, g1, next)
        }

        pub fn encode(bits: &[bool]) -> Vec<bool> {
            let mut state: State = [false; 3];
            let mut out = Vec::with_capacity(bits.len() * 2);
            for &b in bits {
                let (g0, g1, next) = step(state, b);
                out.push(g0);
                out.push(g1);
                state = next;
            }
            out
        }

        pub fn viterbi(
            len: usize,
            terminated: bool,
            cost: impl Fn(usize, bool, bool) -> f32,
        ) -> Option<Vec<bool>> {
            if len == 0 || !len.is_multiple_of(2) {
                return None;
            }
            let steps = len / 2;
            let mut metric = [f32::INFINITY; STATE_COUNT];
            metric[0] = 0.0;
            let mut back: Vec<[Option<(usize, bool)>; STATE_COUNT]> = Vec::with_capacity(steps);
            for k in 0..steps {
                let mut next_metric = [f32::INFINITY; STATE_COUNT];
                let mut step_back: [Option<(usize, bool)>; STATE_COUNT] = [None; STATE_COUNT];
                for (s, &m) in metric.iter().enumerate() {
                    if m.is_infinite() {
                        continue;
                    }
                    for input in [false, true] {
                        let (g0, g1, next) = step(index_to_state(s), input);
                        let candidate = m + cost(k, g0, g1);
                        let next_idx = state_to_index(next);
                        if candidate < next_metric[next_idx] {
                            next_metric[next_idx] = candidate;
                            step_back[next_idx] = Some((s, input));
                        }
                    }
                }
                metric = next_metric;
                back.push(step_back);
            }
            let end = if terminated {
                0
            } else {
                (0..STATE_COUNT).min_by(|&a, &b| metric[a].total_cmp(&metric[b]))?
            };
            if metric[end].is_infinite() {
                return None;
            }
            let mut bits = vec![false; steps];
            let mut state = end;
            for k in (0..steps).rev() {
                let (prev, input) = back[k][state]?;
                bits[k] = input;
                state = prev;
            }
            Some(bits)
        }
    }
}
