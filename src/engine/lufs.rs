//! ITU-R BS.1770 loudness metering for the master bus.

use crate::shared::{Shared, f32_store};

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// ITU-R BS.1770 K-weighted loudness: momentary (400 ms) and short-term (3 s).
pub(super) struct Lufs {
    shelf: [Biquad; 2],
    hp: [Biquad; 2],
    acc: f64,
    count: usize,
    block: usize,
    blocks: [f32; 30],
    idx: usize,
}

impl Lufs {
    pub(super) fn new(sr: f32) -> Self {
        let shelf = {
            let k = (std::f32::consts::PI * 1681.9745 / sr).tan();
            let vh = 10f32.powf(3.9998438 / 20.0);
            let vb = vh.powf(0.49966677);
            let q = 0.70717525;
            let a0 = 1.0 + k / q + k * k;
            Biquad {
                b0: (vh + vb * k / q + k * k) / a0,
                b1: 2.0 * (k * k - vh) / a0,
                b2: (vh - vb * k / q + k * k) / a0,
                a1: 2.0 * (k * k - 1.0) / a0,
                a2: (1.0 - k / q + k * k) / a0,
                ..Default::default()
            }
        };
        let hp = {
            let k = (std::f32::consts::PI * 38.13547 / sr).tan();
            let q = 0.50032705;
            let a0 = 1.0 + k / q + k * k;
            Biquad {
                b0: 1.0,
                b1: -2.0,
                b2: 1.0,
                a1: 2.0 * (k * k - 1.0) / a0,
                a2: (1.0 - k / q + k * k) / a0,
                ..Default::default()
            }
        };
        Lufs {
            shelf: [shelf; 2],
            hp: [hp; 2],
            acc: 0.0,
            count: 0,
            block: (sr * 0.1) as usize,
            blocks: [0.0; 30],
            idx: 0,
        }
    }

    pub(super) fn process(&mut self, l: &[f32], r: &[f32], shared: &Shared) {
        for i in 0..l.len() {
            let a = self.hp[0].tick(self.shelf[0].tick(l[i]));
            let b = self.hp[1].tick(self.shelf[1].tick(r[i]));
            self.acc += (a * a + b * b) as f64;
            self.count += 1;
            if self.count >= self.block {
                self.blocks[self.idx] = (self.acc / self.count as f64) as f32;
                self.idx = (self.idx + 1) % self.blocks.len();
                self.acc = 0.0;
                self.count = 0;
                let n = self.blocks.len();
                let lufs = |ms: f32| -0.691 + 10.0 * ms.max(1e-10).log10();
                let m: f32 = (1..=4).map(|k| self.blocks[(self.idx + n - k) % n]).sum::<f32>() / 4.0;
                let s: f32 = self.blocks.iter().sum::<f32>() / n as f32;
                f32_store(&shared.lufs_momentary, lufs(m));
                f32_store(&shared.lufs_short, lufs(s));
            }
        }
    }
}
