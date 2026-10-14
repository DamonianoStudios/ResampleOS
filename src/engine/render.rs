//! One block of audio, stage by stage. Everything here runs on the audio thread:
//! no allocation, no locks beyond `try_lock` on hosted plugins.

use super::*;

/// How long a bus must go without input before its effect chain is skipped.
const BUS_SLEEP_SECS: f32 = 20.0;

impl Engine {
    pub(super) fn render_block(&mut self, n: usize) {
        if self.flush_in != 0 {
            self.flush_in -= n as i32;
            // Counted down past zero by a block that has already been rendered.
            if self.flush_in <= -(n as i32) {
                self.flush_in = 0;
                // A synth's own release could be seconds long; a panic means now.
                for v in self.synths.iter_mut() {
                    v.active = false;
                }
                for fx in self.fx.iter_mut() {
                    fx.reset();
                }
                self.sc_env = [0.0; NUM_BUSES];
            }
        }
        for b in self.bufs.iter_mut() {
            b[0][..n].fill(0.0);
            b[1][..n].fill(0.0);
        }
        let tps = (self.bpm as f64 / 60.0) * PPQ as f64 / self.sr as f64;
        self.sequence(n as f64 * tps, tps);
        if self.playing {
            if self.song_mode {
                self.apply_song_automation();
            } else {
                self.apply_automation();
            }
        }
        self.render_voices(n);
        self.render_plugins(n);
        self.pull_input(n);
        self.mix_buses(n);
        self.run_analyzers(n);
        self.capture_and_meter(n);
        self.metronome(n);
        self.play_ping(n);
        self.samples_run += n as u64;
    }

    /// Fires every note that starts inside this block, at its exact sample offset.
    /// `span` is the block length in ticks, `tps` ticks per sample.
    fn sequence(&mut self, span: f64, tps: f64) {
        let mut events = std::mem::take(&mut self.events);
        events.clear();
        if self.playing && self.song_mode {
            self.sequence_song(span, tps, &mut events);
            self.song_tick += span;
            self.shared.tick.store(self.tick.to_bits(), Ordering::Relaxed);
            self.shared.song_pos.store(self.song_pos.to_bits(), Ordering::Relaxed);
        } else if self.playing {
            // A block that runs over the end of the loop is sequenced in two parts, with
            // the change of loop (if one is queued) between them.
            let (mut t0, mut left, mut done) = (self.tick, span, 0.0);
            loop {
                let part = left.min((self.loop_end(t0) - t0).max(0.0));
                self.collect_notes(t0, part, done, tps, &mut events);
                t0 += part;
                left -= part;
                done += part;
                if left <= 1e-9 {
                    break;
                }
                let frame = self.shared.input_ring.written() + (done / tps) as u64;
                self.next_scene(frame);
                t0 = 0.0;
            }
            self.tick = t0;
            self.song_tick += span;
            self.song_pos += span;
            self.shared.tick.store(self.tick.to_bits(), Ordering::Relaxed);
            self.shared.song_pos.store(self.song_pos.to_bits(), Ordering::Relaxed);
        }

        // --- Note repeat for held pads.
        let clock = if self.playing { self.tick } else { self.free_tick };
        if self.roll_on {
            let step = self.roll_ticks as f64;
            let next = (clock / step).ceil() * step;
            if next < clock + span {
                let offset = ((next - clock) / tps) as u32;
                for h in self.held.iter().flatten() {
                    if events.len() < events.capacity() {
                        events.push(Event {
                            pad: h.pad,
                            vel: h.vel,
                            semis: h.semis,
                            key: h.key,
                            offset,
                            gate: (step * 0.9 / tps) as i64,
                            locks: crate::drum::NO_LOCKS,
                        });
                    }
                }
            }
        }
        for ev in events.iter() {
            self.trigger(*ev, false);
        }
        self.events = events;
        self.free_tick += span;
    }

    /// Where the stretch of loop starting at `t0` ends: the end of the loop, or the next
    /// bar line if a loop is queued to come in on the bar.
    fn loop_end(&self, t0: f64) -> f64 {
        if self.scene_len == 0 {
            return f64::INFINITY;
        }
        let len = self.scene_len as f64;
        match &self.queued {
            Some(q) if q.next_bar => {
                let bar = TICKS_PER_BAR as f64;
                (((t0 / bar).floor() + 1.0) * bar).min(len)
            }
            _ => len,
        }
    }

    /// Collects the notes starting in `[t0, t0 + span)`. `done` is how many ticks of this
    /// block came before `t0`.
    fn collect_notes(&mut self, t0: f64, span: f64, done: f64, tps: f64, events: &mut Vec<Event>) {
        if span <= 0.0 {
            return;
        }
        for &pad in &self.seq_pads {
            let p = &self.pads[pad as usize];
            let Some(pat) = &p.pattern else { continue };
            if p.cfg.mute || pat.len_ticks == 0 {
                continue;
            }
            pattern_notes(pad, pat, &p.cfg, self.swing, t0, span, done, self.scene_pass, tps, events);
        }
        self.click(t0, span, done, tps);
    }

    /// Schedules the metronome's click if a beat falls in `[t0, t0 + span)`.
    pub(super) fn click(&mut self, t0: f64, span: f64, done: f64, tps: f64) {
        if self.metronome {
            let beat = PPQ as f64;
            let next = (t0 / beat).ceil() * beat;
            if next < t0 + span {
                self.click_at = ((next - t0 + done) / tps) as i64;
                self.click_freq = if (next as u64).is_multiple_of(TICKS_PER_BAR as u64) { 1760.0 } else { 880.0 };
            }
        }
    }

    /// Sample voices and internal synth voices, mixed into their pads' buses.
    fn render_voices(&mut self, n: usize) {
        // --- Voices.
        for &pad in &self.touched {
            f32_store(&self.shared.pad_level[pad as usize], 0.0);
            self.shared.pad_pos[pad as usize].store(u32::MAX, Ordering::Relaxed);
        }
        self.touched.clear();

        let sr = self.sr;
        // Synced LFOs follow the transport, and keep running at the tempo when it is stopped.
        let beat_ticks = if self.playing { self.song_tick } else { self.free_tick };
        let seconds = self.samples_run as f64 / sr as f64;
        self.shared.lfo_ticks.store(beat_ticks.to_bits(), Ordering::Relaxed);
        self.shared.lfo_secs.store(seconds.to_bits(), Ordering::Relaxed);
        let any_solo = self.any_solo;
        let Engine { voices, synths, drums, pads, bufs, scratch, touched, shared, .. } = self;

        for v in voices.iter_mut().filter(|v| v.active) {
            let pad = &pads[v.pad as usize];
            let cfg = &pad.cfg;
            let Some(s) = &pad.sample else {
                v.active = false;
                continue;
            };
            let len = s.len();
            let end = if cfg.end == 0 || cfg.end as usize > len { len } else { cfg.end as usize };
            let start = (cfg.start as usize).min(end);
            if end - start < 2 {
                v.active = false;
                continue;
            }
            let lfo = if cfg.lfo_target == LfoTarget::Off {
                0.0
            } else {
                cfg.lfo_shape.at(lfo_phase(cfg, beat_ticks, seconds)) * cfg.lfo_depth
            };
            let audible = !cfg.mute && (!any_solo || cfg.solo);
            let mut vol = if audible { cfg.vol * v.gain } else { 0.0 };
            let mut pan = cfg.pan;
            let mut pitch = cfg.pitch;
            let mut cutoff = cfg.cutoff;
            match cfg.lfo_target {
                LfoTarget::Pitch => pitch += lfo * 12.0,
                LfoTarget::Volume => vol *= 1.0 - (lfo * 0.5 + 0.5 * cfg.lfo_depth),
                LfoTarget::Pan => pan = (pan + lfo).clamp(-1.0, 1.0),
                LfoTarget::Filter => cutoff = (cutoff + lfo * 0.5).clamp(0.0, 1.0),
                LfoTarget::Off => {}
            }
            let rate = v.rate * 2f64.powf(pitch as f64 / 12.0) * if cfg.reverse { -1.0 } else { 1.0 };
            let (gl, gr) = pan_gains(pan, vol);
            let filter = cutoff < 0.995;
            let g = svf_g(20.0 * 1000f32.powf(cutoff), sr);
            let k = 1.0 / (0.7 + cfg.reso * 11.0);
            let att = 1.0 / (cfg.attack.max(0.0005) * sr);
            let rel = 1.0 / (cfg.release.max(0.002) * sr);
            let kill = 1.0 / (0.004 * sr);
            let looping = cfg.mode == PlayMode::Loop;
            let span = (end - start) as f64;
            let out = &mut bufs[bus_index(cfg.bus)];
            let (ol, or) = out.split_at_mut(1);
            let (ol, or) = (&mut ol[0], &mut or[0]);

            let first = (v.offset as usize).min(n);
            v.offset -= first as u32;
            for i in first..n {
                if v.gate_left >= 0 {
                    if v.gate_left == 0 {
                        v.releasing = true;
                    }
                    v.gate_left -= 1;
                }
                if rate >= 0.0 {
                    if v.pos >= end as f64 {
                        if looping {
                            v.pos -= span;
                        } else {
                            v.active = false;
                            break;
                        }
                    }
                } else if v.pos < start as f64 {
                    if looping {
                        v.pos += span;
                    } else {
                        v.active = false;
                        break;
                    }
                }
                let mut a = hermite(&s.l, v.pos);
                let mut b = hermite(&s.r, v.pos);

                if v.killed {
                    v.env -= kill;
                } else if v.releasing {
                    v.env -= rel;
                } else if v.env < 1.0 {
                    v.env = (v.env + att).min(1.0);
                }
                if v.env <= 0.0 && (v.killed || v.releasing) {
                    v.active = false;
                    break;
                }
                // Short fade at the cut point so slices don't click when they run out.
                let edge = if looping {
                    1.0
                } else if rate >= 0.0 {
                    ((end as f64 - v.pos) / 48.0).min(1.0) as f32
                } else {
                    ((v.pos - start as f64) / 48.0).clamp(0.0, 1.0) as f32
                };
                if filter {
                    a = v.svf[0].tick(a, g, k).0;
                    b = v.svf[1].tick(b, g, k).0;
                }
                let e = v.env * edge;
                ol[i] += a * gl * e;
                or[i] += b * gr * e;
                v.pos += rate;
            }
            if v.active {
                report_level(shared, touched, v.pad, v.env.max(0.05));
                shared.pad_pos[v.pad as usize].store(v.pos.max(0.0) as u32, Ordering::Relaxed);
            }
        }

        for v in synths.iter_mut().filter(|v| v.active) {
            let cfg = &pads[v.pad as usize].cfg;
            let audible = !cfg.mute && (!any_solo || cfg.solo);
            scratch[..n].fill(0.0);
            v.render(&cfg.synth, sr, &mut scratch[..n]);
            let vol = if audible { cfg.vol } else { 0.0 };
            let (gl, gr) = pan_gains(cfg.pan, vol);
            let out = &mut bufs[bus_index(cfg.bus)];
            for i in 0..n {
                out[0][i] += scratch[i] * gl;
                out[1][i] += scratch[i] * gr;
            }
            if v.active {
                report_level(shared, touched, v.pad, v.level().max(0.05));
            }
        }

        for v in drums.iter_mut().filter(|v| v.active) {
            let cfg = &pads[v.pad as usize].cfg;
            let audible = !cfg.mute && (!any_solo || cfg.solo);
            let (gl, gr) = pan_gains(cfg.pan, if audible { cfg.vol } else { 0.0 });
            let out = &mut bufs[bus_index(cfg.bus)];
            let (l, r) = out.split_at_mut(1);
            v.render(&mut l[0][..n], &mut r[0][..n], gl, gr);
            if v.active {
                report_level(shared, touched, v.pad, v.loudness().clamp(0.05, 1.0));
            }
        }
    }

    fn render_plugins(&mut self, n: usize) {
        // --- Hosted instruments: end notes whose gate ran out, then render each plugin.
        for i in 0..self.plug_notes.len() {
            let Some(mut pn) = self.plug_notes[i] else { continue };
            if pn.left < 0 {
                continue;
            }
            if pn.left < n as i64 {
                if let Some(rt) = self.pad_plugins[pn.pad as usize].as_mut() {
                    rt.push(PlugEv { on: false, note: pn.note, vel: 0, offset: pn.left as u32 });
                }
                self.plug_notes[i] = None;
            } else {
                pn.left -= n as i64;
                self.plug_notes[i] = Some(pn);
            }
        }
        for k in 0..self.plugin_pads.len() {
            let pad = self.plugin_pads[k] as usize;
            let cfg = self.pads[pad].cfg;
            let Some(rt) = self.pad_plugins[pad].as_mut() else { continue };
            rt.prepare(n);
            if !rt.run(n) || rt.buf.outputs.is_empty() {
                continue;
            }
            let audible = !cfg.mute && (!self.any_solo || cfg.solo);
            let vol = if audible { cfg.vol } else { 0.0 };
            // Balance rather than pan law: the plugin's output is already a stereo image.
            let (gl, gr) = (vol * (1.0 - cfg.pan).min(1.0), vol * (1.0 + cfg.pan).min(1.0));
            let outs = &rt.buf.outputs;
            let (a, b) = (&outs[0], &outs[outs.len().min(2) - 1]);
            let dest = &mut self.bufs[bus_index(cfg.bus)];
            let mut peak = 0f32;
            for i in 0..n.min(a.len()).min(b.len()) {
                // A misbehaving plugin must not poison the mix with NaN or infinity.
                let (x, y) = (a[i], b[i]);
                if x.is_finite() && y.is_finite() {
                    dest[0][i] += x * gl;
                    dest[1][i] += y * gr;
                    peak = peak.max(x.abs()).max(y.abs());
                }
            }
            if peak > 1e-4 {
                report_level(&self.shared, &mut self.touched, pad as u16, peak.min(1.0));
            }
        }
    }

    fn pull_input(&mut self, n: usize) {
        // --- External input: pulled from the capture thread's ring, rate-matched, and
        // mixed into its monitor bus so it can be coloured, resampled and skip-backed.
        self.in_buf[0][..n].fill(0.0);
        self.in_buf[1][..n].fill(0.0);
        self.read_input(n);
        // Written even when silent, so positions in this ring always follow the clock.
        self.shared.input_ring.write(&self.in_buf[0][..n], &self.in_buf[1][..n], &mut self.in_ring_peak);
    }

    fn read_input(&mut self, n: usize) {
        if let Some(rx) = self.input.as_mut() {
            // If the two devices' clocks drift apart, shed the backlog instead of lagging.
            let limit = (self.sr as f64 * self.in_ratio * 0.2) as usize * 2;
            if rx.slots() > limit {
                let drop = rx.slots() - limit / 4;
                for _ in 0..drop {
                    let _ = rx.pop();
                }
            }
            let mut peak = 0f32;
            let env_fall = (-1.0 / (0.3 * self.sr)).exp();
            let fold_step = 1.0 - (-1.0 / (0.005 * self.sr)).exp();
            for i in 0..n {
                self.in_phase += self.in_ratio;
                while self.in_phase >= 1.0 {
                    self.in_phase -= 1.0;
                    self.in_prev = self.in_next;
                    self.in_next = match (rx.pop(), rx.pop()) {
                        (Ok(l), Ok(r)) => [l, r],
                        _ => [0.0; 2],
                    };
                }
                let t = self.in_phase as f32;
                let l = (self.in_prev[0] + (self.in_next[0] - self.in_prev[0]) * t) * self.in_gain;
                let r = (self.in_prev[1] + (self.in_next[1] - self.in_prev[1]) * t) * self.in_gain;
                // A microphone on one input of a stereo interface arrives on one side only.
                // Fold the sides together when that is what is coming in, so it is heard
                // and recorded in the middle. The verdict holds through silences.
                self.in_env[0] = l.abs().max(self.in_env[0] * env_fall);
                self.in_env[1] = r.abs().max(self.in_env[1] * env_fall);
                let (lo, hi) = (self.in_env[0].min(self.in_env[1]), self.in_env[0].max(self.in_env[1]));
                if hi > 0.003 {
                    self.in_one_sided = lo < hi * 0.02;
                }
                let want = match self.in_mode {
                    1 => 0.0,
                    2 => 1.0,
                    _ => self.in_one_sided as u8 as f32,
                };
                self.in_fold += (want - self.in_fold) * fold_step;
                let (l, r) = (l + r * self.in_fold, r + l * self.in_fold);
                self.in_buf[0][i] = l;
                self.in_buf[1][i] = r;
                peak = peak.max(l.abs()).max(r.abs());
            }
            self.in_peak = (self.in_peak * 0.99995f32.powi(n as i32)).max(peak);
            f32_store(&self.shared.input_peak, self.in_peak);
            if self.in_dest > 0 {
                let dest = &mut self.bufs[bus_index(self.in_dest - 1)];
                for i in 0..n {
                    dest[0][i] += self.in_buf[0][i];
                    dest[1][i] += self.in_buf[1][i];
                }
            }
        }
    }

    fn mix_buses(&mut self, n: usize) {
        // Sidechain keys are read before any bus is processed, so a key is always "what is
        // being sent into that bus" regardless of the order buses are mixed in.
        for src in 0..MASTER {
            if (0..NUM_BUSES).any(|b| self.buses[b].sidechain_source(b) == Some(src)) {
                let (l, r) = (&self.bufs[src][0], &self.bufs[src][1]);
                for i in 0..n {
                    self.key_buf[src][i] = l[i].abs().max(r[i].abs());
                }
            }
        }

        let ctx = FxCtx { bpm: self.bpm, scale: self.scale };
        // How present the vocal is, read from what is sent into its bus, for the bus that
        // makes room for it. Like the sidechain keys, taken before anything is processed.
        let duck_bus = (self.vocal_fx.duck_bus as usize).checked_sub(1).filter(|b| *b < VOCAL);
        if duck_bus.is_some() {
            let (l, r) = (&self.bufs[VOCAL][0], &self.bufs[VOCAL][1]);
            for i in 0..n {
                self.vox_key[i] = l[i].abs().max(r[i].abs());
            }
        }
        // --- Buses in order, so a bus can feed any later bus, then the master.
        for b in 0..NUM_BUSES {
            let cfg = self.buses[b];
            let (head, tail) = self.bufs.split_at_mut(b + 1);
            let buf = &mut head[b];
            {
                let (l, r) = buf.split_at_mut(1);
                let (l, r) = (&mut l[0][..n], &mut r[0][..n]);
                // A bus with no input for longer than any effect tail has nothing to say;
                // skipping its chain keeps fifteen idle slots from costing CPU. Hosted
                // plugins are exempt, since some generate sound on their own.
                let hosts_plugin = cfg.slots.iter().any(|s| s.on && s.kind == FxKind::Vst);
                if l.iter().chain(r.iter()).any(|v| *v != 0.0) || hosts_plugin {
                    self.bus_idle[b] = 0;
                } else {
                    self.bus_idle[b] = self.bus_idle[b].saturating_add(n as u32);
                }
                let asleep = self.bus_idle[b] as f32 > self.sr * BUS_SLEEP_SECS;
                if b == VOCAL && !asleep {
                    vocal_front_end(&mut self.vox, &self.vocal_fx, self.sr, l, r);
                }
                for (s, slot) in cfg.slots.iter().enumerate() {
                    if asleep || !slot.on || slot.kind == FxKind::None {
                        continue;
                    }
                    if slot.kind != FxKind::Vst {
                        self.fx[b * 3 + s].process(slot.kind, slot.params, slot.sliders, ctx, l, r);
                        continue;
                    }
                    let Some(rt) = self.fx_plugins[b * 3 + s].as_mut() else { continue };
                    rt.prepare(n);
                    feed_stereo(&mut rt.buf.inputs, l, r);
                    let input_peak = l[..n].iter().chain(&r[..n]).fold(0f32, |m, v| m.max(v.abs()));
                    // A busy or failed plugin passes the dry signal through untouched, and so
                    // does one that has gone silent.
                    let ran = rt.run(n);
                    let silent = ran && rt.gone_silent(n, input_peak, self.sr);
                    self.shared.fx_silent[b * 3 + s].store(silent, Ordering::Relaxed);
                    if ran && !silent && !rt.buf.outputs.is_empty() {
                        let outs = &rt.buf.outputs;
                        let (a, b2) = (&outs[0], &outs[outs.len().min(2) - 1]);
                        let mix = slot.params[0];
                        for i in 0..n.min(a.len()).min(b2.len()) {
                            if a[i].is_finite() && b2[i].is_finite() {
                                l[i] += (a[i] - l[i]) * mix;
                                r[i] += (b2[i] - r[i]) * mix;
                            }
                        }
                    }
                }
                if let Some(src) = cfg.sidechain_source(b) {
                    let key = &self.key_buf[src];
                    let attack = 1.0 - (-1.0 / (0.002 * self.sr)).exp();
                    let release = (-1.0 / (cfg.sc_release.clamp(0.01, 2.0) * self.sr)).exp();
                    let amount = cfg.sc_amount.clamp(-1.0, 1.0);
                    let mut env = self.sc_env[b];
                    for i in 0..n {
                        if key[i] > env {
                            env += attack * (key[i] - env);
                        } else {
                            env *= release;
                        }
                        // A key peaking around -12 dBFS already drives the effect fully.
                        let e = (env * 4.0).min(1.0);
                        let g = if amount < 0.0 { 1.0 + amount * e } else { 1.0 - amount + amount * e };
                        l[i] *= g;
                        r[i] *= g;
                    }
                    self.sc_env[b] = env;
                }
                if duck_bus == Some(b) {
                    make_room(&mut self.vox, &self.vox_key[..n], self.sr, l, r);
                }
                let gain = if cfg.mute { 0.0 } else { cfg.level };
                // Balance rather than pan law: what is on a bus is already a stereo image.
                let pan = cfg.pan.clamp(-1.0, 1.0);
                let (gl, gr) = (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0));
                let (mut pl, mut pr) = (0f32, 0f32);
                for i in 0..n {
                    l[i] *= gl;
                    r[i] *= gr;
                    pl = pl.max(l[i].abs());
                    pr = pr.max(r[i].abs());
                }
                // Meter ballistics live here so the UI can't miss a peak between frames.
                let decay = 0.99995f32.powi(n as i32);
                self.peaks[b * 2] = (self.peaks[b * 2] * decay).max(pl);
                self.peaks[b * 2 + 1] = (self.peaks[b * 2 + 1] * decay).max(pr);
                f32_store(&self.shared.bus_peak[b * 2], self.peaks[b * 2]);
                f32_store(&self.shared.bus_peak[b * 2 + 1], self.peaks[b * 2 + 1]);
            }
            if b < MASTER {
                let dest = if cfg.out as usize > b + 1 && (cfg.out as usize) <= MASTER {
                    cfg.out as usize - 1
                } else {
                    MASTER
                };
                let d = &mut tail[dest - b - 1];
                for i in 0..n {
                    d[0][i] += buf[0][i];
                    d[1][i] += buf[1][i];
                }
            }
        }
    }

    /// Feeds each analyzer plugin a copy of the bus it watches. Their output is ignored.
    fn run_analyzers(&mut self, n: usize) {
        for (slot, source) in self.analyzers.iter_mut() {
            let Some(rt) = slot.as_mut() else { continue };
            let [l, r] = &self.bufs[*source as usize];
            rt.prepare(n);
            feed_stereo(&mut rt.buf.inputs, &l[..n], &r[..n]);
            rt.run(n);
        }
    }

    /// Feeds the skip-back ring from the chosen source and meters the master.
    fn capture_and_meter(&mut self, n: usize) {
        if self.ring_src == 5 {
            self.shared.ring.write(&self.in_buf[0][..n], &self.in_buf[1][..n], &mut self.ring_peak);
        } else {
            let src = match self.ring_src {
                0 => MASTER,
                6 => VOCAL,
                s => s as usize - 1,
            };
            self.shared.ring.write(&self.bufs[src][0][..n], &self.bufs[src][1][..n], &mut self.ring_peak);
        }
        {
            let m = &self.bufs[MASTER];
            self.lufs.process(&m[0][..n], &m[1][..n], &self.shared);
        }
    }

    /// The latency test signal goes straight to the output, after everything else. The
    /// input ring's position for its first sample is published for the UI to look from.
    fn play_ping(&mut self, n: usize) {
        if self.ping_pos >= self.ping.len() {
            return;
        }
        if self.ping_pos == 0 {
            // This block's input is already in the ring, so its first frame is n back.
            let frame = self.shared.input_ring.written().saturating_sub(n as u64);
            self.shared.ping_frame.store(frame, Ordering::Relaxed);
        }
        let [l, r] = &mut self.bufs[MASTER];
        let k = n.min(self.ping.len() - self.ping_pos);
        for ((l, r), s) in l.iter_mut().zip(r.iter_mut()).zip(&self.ping[self.ping_pos..self.ping_pos + k]) {
            *l += s;
            *r += s;
        }
        self.ping_pos += k;
    }

    fn metronome(&mut self, n: usize) {
        // --- Metronome goes in after capture so it never ends up in a resample.
        if self.click_at >= 0 || self.click_env > 1e-4 {
            let m = &mut self.bufs[MASTER];
            for i in 0..n {
                if self.click_at == i as i64 {
                    self.click_env = 0.5;
                    self.click_phase = 0.0;
                }
                if self.click_env > 1e-4 {
                    self.click_phase = (self.click_phase + self.click_freq / self.sr).fract();
                    let s = (self.click_phase * TAU).sin() * self.click_env;
                    self.click_env *= 0.9985;
                    m[0][i] += s;
                    m[1][i] += s;
                }
            }
            self.click_at = -1;
        }
    }
}

/// Puts a stereo pair into a plugin's inputs: both sides, or their mix for a mono input.
fn feed_stereo(inputs: &mut [Vec<f32>], l: &[f32], r: &[f32]) {
    match inputs {
        [] => {}
        [mono] => {
            for (o, (a, b)) in mono.iter_mut().zip(l.iter().zip(r)) {
                *o = (a + b) * 0.5;
            }
        }
        [il, ir, ..] => {
            il[..l.len()].copy_from_slice(l);
            ir[..r.len()].copy_from_slice(r);
        }
    }
}

/// The vocal bus's gate and de-esser, ahead of its effect slots.
fn vocal_front_end(st: &mut VocalState, cfg: &VocalFx, sr: f32, l: &mut [f32], r: &mut [f32]) {
    let gating = cfg.gate_db > -79.5;
    if !gating && cfg.deess <= 0.001 {
        return;
    }
    let open_at = 10f32.powf(cfg.gate_db / 20.0);
    let env_fall = (-1.0 / (0.08 * sr)).exp();
    let (gain_up, gain_down) = (1.0 - (-1.0 / (0.002 * sr)).exp(), 1.0 - (-1.0 / (0.15 * sr)).exp());
    let split = svf_g(5500.0, sr);
    let follow = 1.0 - (-1.0 / (0.004 * sr)).exp();
    for i in 0..l.len() {
        if gating {
            let level = l[i].abs().max(r[i].abs());
            st.gate_env = level.max(st.gate_env * env_fall);
            // Closes a little below where it opens, so a level sitting on the threshold
            // does not chatter.
            st.gate_open = st.gate_env > if st.gate_open { open_at * 0.6 } else { open_at };
            let want = if st.gate_open { 1.0 } else { 0.0 };
            st.gate_gain += (want - st.gate_gain) * if st.gate_open { gain_up } else { gain_down };
            l[i] *= st.gate_gain;
            r[i] *= st.gate_gain;
        }
        if cfg.deess > 0.001 {
            let (_, _, hl) = st.ess[0].tick(l[i], split, 1.414);
            let (_, _, hr) = st.ess[1].tick(r[i], split, 1.414);
            st.ess_hi += follow * (hl.abs().max(hr.abs()) - st.ess_hi);
            st.ess_all += follow * (l[i].abs().max(r[i].abs()) - st.ess_all);
            // Sibilance is where the top end makes up most of the signal.
            let excess = (st.ess_hi / (st.ess_all + 1e-6) - 0.35).max(0.0);
            let cut = (excess * cfg.deess * 4.0).min(0.85);
            l[i] -= hl * cut;
            r[i] -= hr * cut;
        }
    }
}

/// Turns the midrange (300 Hz to 3.5 kHz) of a bus down by up to 6 dB while the vocal is
/// present, leaving its lows and highs alone.
fn make_room(st: &mut VocalState, key: &[f32], sr: f32, l: &mut [f32], r: &mut [f32]) {
    let (rise, fall) = (1.0 - (-1.0 / (0.015 * sr)).exp(), (-1.0 / (0.25 * sr)).exp());
    let (top, bottom) = (svf_g(3500.0, sr), svf_g(300.0, sr));
    for i in 0..l.len() {
        // A vocal around -20 dBFS already counts as fully present.
        let level = (key[i] * 10.0).min(1.0);
        st.presence = if level > st.presence { st.presence + rise * (level - st.presence) } else { st.presence * fall };
        let cut = 0.5 * st.presence;
        let (low, _, _) = st.duck[0].tick(l[i], top, 1.414);
        let (_, _, mid) = st.duck[1].tick(low, bottom, 1.414);
        l[i] -= mid * cut;
        let (low, _, _) = st.duck[2].tick(r[i], top, 1.414);
        let (_, _, mid) = st.duck[3].tick(low, bottom, 1.414);
        r[i] -= mid * cut;
    }
}

/// Largest timing drift at full humanize, in ticks: a sixteenth of a beat either way.
const HUMAN_TICKS: f64 = 30.0;

/// Two values in -1..1 that depend only on which note of which pass this is, so a note's
/// drift is the same no matter which audio block happens to ask about it. Each is the mean
/// of two uniform values, which clusters near zero the way a player's timing does.
fn jitter(pad: u16, note: usize, pass: i64) -> (f32, f32) {
    let mut h = ((pad as u64) << 44) ^ ((note as u64) << 22) ^ pass as u64;
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    let u = |shift: u32| ((h >> shift) & 0xffff) as f32 / 65535.0;
    (u(0) + u(16) - 1.0, u(32) + u(48) - 1.0)
}

/// Sequencer path for pads with humanize on. Notes are placed on the absolute timeline so
/// one that drifts across a loop boundary still fires exactly once.
/// Collects the notes of `pat`, read as a loop, that start in `[t0, t0 + span)` of it.
/// `done` is how many ticks of the block came before `t0`, and `pass` tells one time
/// round from another, for humanize.
#[allow(clippy::too_many_arguments)]
pub(super) fn pattern_notes(pad: u16, pat: &Pattern, cfg: &PadCfg, swing: f32, t0: f64, span: f64, done: f64, pass: i64, tps: f64, events: &mut Vec<Event>) {
    if cfg.human_time > 0.0 || cfg.human_vel > 0.0 {
        humanized_notes(pad, pat, cfg, swing, t0, span, done, pass, tps, events);
        return;
    }
    let len = pat.len_ticks as f64;
    let a = t0.rem_euclid(len);
    let b = a + span;
    for note in &pat.notes {
        let mut w = swing_tick(note.start as f64, swing);
        if w < a {
            w += len;
        }
        if w >= a && w < b && events.len() < events.capacity() {
            events.push(Event {
                pad,
                vel: note.vel as f32 / 127.0,
                semis: note.pitch as f32 - 60.0,
                key: 0x8000_0000 | note.pitch as u32,
                offset: ((w - a + done) / tps) as u32,
                gate: (note.len as f64 / tps) as i64,
                locks: note.locks,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn humanized_notes(
    pad: u16,
    pat: &Pattern,
    cfg: &PadCfg,
    swing: f32,
    t0: f64,
    span: f64,
    done: f64,
    scene_pass: i64,
    tps: f64,
    events: &mut Vec<Event>,
) {
    let len = pat.len_ticks as f64;
    let pass0 = (t0 / len).floor() as i64;
    for (idx, note) in pat.notes.iter().enumerate() {
        let base = swing_tick(note.start as f64, swing);
        for pass in (pass0 - 1).max(0)..=pass0 + 1 {
            // Each time round the loop counts as a new pass too.
            let (jt, jv) = jitter(pad, idx, pass + scene_pass * 4099);
            let at = (pass as f64 * len + base + jt as f64 * cfg.human_time as f64 * HUMAN_TICKS).max(0.0);
            if at >= t0 && at < t0 + span && events.len() < events.capacity() {
                let vel = note.vel as f32 / 127.0 * (1.0 + jv * cfg.human_vel * 0.5);
                events.push(Event {
                    pad,
                    vel: vel.clamp(0.05, 1.0),
                    semis: note.pitch as f32 - 60.0,
                    key: 0x8000_0000 | note.pitch as u32,
                    offset: ((at - t0 + done) / tps) as u32,
                    gate: (note.len as f64 / tps) as i64,
                    locks: note.locks,
                });
            }
        }
    }
}

/// Equal-power pan for a mono-summed source: unity on both sides at centre.
#[inline]
fn pan_gains(pan: f32, vol: f32) -> (f32, f32) {
    let angle = (pan + 1.0) * FRAC_PI_4;
    (angle.cos() * SQRT_2 * vol, angle.sin() * SQRT_2 * vol)
}

/// Marks `pad` as sounding at `level` for the UI, remembering it so the next block can
/// clear the mark if the pad has gone quiet.
fn report_level(shared: &Shared, touched: &mut Vec<u16>, pad: u16, level: f32) {
    let slot = &shared.pad_level[pad as usize];
    let cur = f32_load(slot);
    if cur == 0.0 {
        touched.push(pad);
    }
    f32_store(slot, cur.max(level));
}
