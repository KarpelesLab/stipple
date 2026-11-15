//! The game itself, without drawing or sound: a fixed-step simulation on
//! `f32` with no trigonometry, so a seeded run plays out the same on every
//! machine (the selftest checks its exact score).

/// The virtual screen, in pixels.
pub const W: f32 = 320.0;
pub const H: f32 = 180.0;
/// The top band the HUD covers.
pub const TOP: f32 = 18.0;
pub const MAX_HEALTH: u32 = 5;
pub const MAX_LIGHT: f32 = 100.0;
/// Light spent by a flare, and the least it takes.
pub const FLARE_COST: f32 = 18.0;
const LIGHT_DECAY: f32 = 3.5;
const EMBER_LIGHT: f32 = 14.0;
const PLAYER_SPEED: f32 = 78.0;
const HIT_RADIUS: f32 = 8.0;
const PICK_RADIUS: f32 = 11.0;
const INVULNERABLE: f32 = 1.4;
const COMBO_WINDOW: f32 = 1.6;
const COLD_EVERY: f32 = 3.0;

/// xorshift64*: small, seeded, the same everywhere.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }

    /// In `[0, 1)`, exactly representable.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

impl V2 {
    pub const fn new(x: f32, y: f32) -> V2 {
        V2 { x, y }
    }
    pub fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn minus(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
    pub fn plus(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
    pub fn scale(self, k: f32) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
    pub fn norm(self) -> V2 {
        let l = self.len();
        if l > 1e-6 {
            self.scale(1.0 / l)
        } else {
            V2::default()
        }
    }
}

/// A triangle wave in -1..1 with period 1: a sway without `sin`.
fn tri(p: f32) -> f32 {
    4.0 * ((p - p.floor()) - 0.5).abs() - 1.0
}

#[derive(Clone, Debug)]
pub struct Ember {
    pub pos: V2,
    base_x: f32,
    fall: f32,
    phase: f32,
    pub age: f32,
}

#[derive(Clone, Debug)]
pub struct Moth {
    pub pos: V2,
    pub vel: V2,
    pub age: f32,
    /// Drawn toward the lantern (it's close enough to see it).
    pub drawn: bool,
}

/// What a step's input asks for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Control {
    /// The direction to walk, no longer than 1.
    pub dir: V2,
    pub flare: bool,
}

/// What happened in a step, for sound, text and shaking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Ember { at: V2, points: u32, combo: u32 },
    Hit { at: V2 },
    Cold { at: V2 },
    Flare { at: V2, burst: u32 },
    Fizzle,
    Over,
}

#[derive(Clone, Debug)]
pub struct World {
    rng: Rng,
    pub time: f32,
    pub player: V2,
    /// -1 facing left, 1 right.
    pub facing: f32,
    pub walking: bool,
    pub light: f32,
    pub health: u32,
    pub score: u32,
    pub combo: u32,
    combo_left: f32,
    pub hurt: f32,
    cold: f32,
    flare_cooldown: f32,
    /// Seconds since the last flare (for its ring).
    pub flare_age: f32,
    pub embers: Vec<Ember>,
    pub moths: Vec<Moth>,
    next_ember: f32,
    next_moth: f32,
    pub caught: u32,
    pub burst: u32,
    pub over: bool,
}

impl World {
    pub fn new(seed: u64) -> World {
        World {
            rng: Rng::new(seed),
            time: 0.0,
            player: V2::new(W / 2.0, H * 0.62),
            facing: 1.0,
            walking: false,
            light: 70.0,
            health: MAX_HEALTH,
            score: 0,
            combo: 0,
            combo_left: 0.0,
            hurt: 0.0,
            cold: 0.0,
            flare_cooldown: 0.0,
            flare_age: 10.0,
            embers: Vec::new(),
            moths: Vec::new(),
            next_ember: 0.4,
            next_moth: 2.5,
            caught: 0,
            burst: 0,
            over: false,
        }
    }

    /// The lantern's reach, in pixels.
    pub fn radius(&self) -> f32 {
        22.0 + self.light * 0.5
    }

    /// One step of `dt` seconds.
    pub fn step(&mut self, ctl: Control, dt: f32, events: &mut Vec<Event>) {
        if self.over {
            return;
        }
        self.time += dt;
        self.hurt = (self.hurt - dt).max(0.0);
        self.flare_cooldown = (self.flare_cooldown - dt).max(0.0);
        self.flare_age += dt;
        self.combo_left -= dt;
        if self.combo_left <= 0.0 {
            self.combo = 0;
        }

        // Walk.
        let d = if ctl.dir.len() > 1.0 {
            ctl.dir.norm()
        } else {
            ctl.dir
        };
        self.walking = d.len() > 0.05;
        if d.x.abs() > 0.05 {
            self.facing = d.x.signum();
        }
        let p = self.player.plus(d.scale(PLAYER_SPEED * dt));
        self.player = V2::new(p.x.clamp(8.0, W - 8.0), p.y.clamp(TOP + 8.0, H - 8.0));

        // The lantern burns down; at its lowest the cold bites.
        self.light = (self.light - LIGHT_DECAY * dt).max(0.0);
        if self.light <= 0.0 {
            self.cold += dt;
            if self.cold >= COLD_EVERY {
                self.cold = 0.0;
                self.damage(events, Event::Cold { at: self.player });
            }
        } else {
            self.cold = 0.0;
        }

        // A flare: spend light, burst the moths in reach.
        if ctl.flare && self.flare_cooldown <= 0.0 {
            if self.light >= FLARE_COST {
                let reach = self.radius() + 12.0;
                self.light -= FLARE_COST;
                self.flare_cooldown = 0.8;
                self.flare_age = 0.0;
                let player = self.player;
                let before = self.moths.len();
                self.moths.retain(|m| m.pos.minus(player).len() > reach);
                let burst = (before - self.moths.len()) as u32;
                self.burst += burst;
                self.score += 25 * burst;
                events.push(Event::Flare { at: player, burst });
            } else {
                self.flare_cooldown = 0.3;
                events.push(Event::Fizzle);
            }
        }

        self.spawn(dt);

        // Embers drift down, swaying; caught ones feed the lantern.
        let mut i = 0;
        while i < self.embers.len() {
            let e = &mut self.embers[i];
            e.age += dt;
            e.pos.y += e.fall * dt;
            e.pos.x = e.base_x + 10.0 * tri(e.phase + e.age * 0.35);
            let at = e.pos;
            if at.minus(self.player).len() < PICK_RADIUS {
                self.embers.swap_remove(i);
                self.combo = (self.combo + 1).min(5);
                self.combo_left = COMBO_WINDOW;
                let points = 10 * self.combo;
                self.score += points;
                self.caught += 1;
                self.light = (self.light + EMBER_LIGHT).min(MAX_LIGHT);
                events.push(Event::Ember {
                    at,
                    points,
                    combo: self.combo,
                });
            } else if at.y > H + 6.0 {
                self.embers.swap_remove(i);
            } else {
                i += 1;
            }
        }

        // Moths wander until they see the lantern, then home in on it.
        let reach = self.radius() * 1.7;
        let speed = 26.0 + self.time.min(120.0) * 0.12;
        let mut hit = None;
        for (i, m) in self.moths.iter_mut().enumerate() {
            m.age += dt;
            let to = self.player.minus(m.pos);
            m.drawn = to.len() < reach;
            if m.drawn {
                let want = to.norm().scale(speed);
                let k = (2.2 * dt).min(1.0);
                m.vel = m.vel.plus(want.minus(m.vel).scale(k));
            }
            // A flutter across the path.
            let flutter = tri(m.age * 1.7) * 9.0;
            let side = V2::new(-m.vel.y, m.vel.x).norm().scale(flutter * dt);
            m.pos = m.pos.plus(m.vel.scale(dt)).plus(side);
            if !m.drawn {
                if m.pos.x < 4.0 || m.pos.x > W - 4.0 {
                    m.vel.x = -m.vel.x;
                }
                if m.pos.y < TOP || m.pos.y > H - 4.0 {
                    m.vel.y = -m.vel.y;
                }
                m.pos = V2::new(m.pos.x.clamp(0.0, W), m.pos.y.clamp(TOP - 4.0, H));
            }
            if hit.is_none() && to.len() < HIT_RADIUS && self.hurt <= 0.0 {
                hit = Some(i);
            }
        }
        if let Some(i) = hit {
            let at = self.moths.swap_remove(i).pos;
            self.damage(events, Event::Hit { at });
        }
    }

    fn damage(&mut self, events: &mut Vec<Event>, event: Event) {
        self.hurt = INVULNERABLE;
        self.combo = 0;
        self.health = self.health.saturating_sub(1);
        events.push(event);
        if self.health == 0 {
            self.over = true;
            events.push(Event::Over);
        }
    }

    fn spawn(&mut self, dt: f32) {
        self.next_ember -= dt;
        if self.next_ember <= 0.0 && self.embers.len() < 9 {
            self.next_ember = self.rng.range(0.6, 1.3);
            let x = self.rng.range(20.0, W - 20.0);
            self.embers.push(Ember {
                pos: V2::new(x, TOP - 4.0),
                base_x: x,
                fall: self.rng.range(11.0, 20.0),
                phase: self.rng.unit(),
                age: 0.0,
            });
        }
        self.next_moth -= dt;
        if self.next_moth <= 0.0 && self.moths.len() < 12 {
            self.next_moth = (3.0 - self.time * 0.035).max(0.8) * self.rng.range(0.7, 1.3);
            // From a side edge or the bottom, flying inward.
            let edge = self.rng.next_u32() % 3;
            let (pos, vel) = match edge {
                0 => (
                    V2::new(-4.0, self.rng.range(TOP + 10.0, H - 10.0)),
                    V2::new(18.0, 0.0),
                ),
                1 => (
                    V2::new(W + 4.0, self.rng.range(TOP + 10.0, H - 10.0)),
                    V2::new(-18.0, 0.0),
                ),
                _ => (
                    V2::new(self.rng.range(20.0, W - 20.0), H + 4.0),
                    V2::new(0.0, -18.0),
                ),
            };
            let vel = vel.plus(V2::new(
                self.rng.range(-8.0, 8.0),
                self.rng.range(-8.0, 8.0),
            ));
            self.moths.push(Moth {
                pos,
                vel,
                age: self.rng.unit(),
                drawn: false,
            });
        }
    }
}

/// A scripted player for the selftest: walk to the nearest ember, away from
/// close moths, flare when two or more are near. After `tired_at` seconds it
/// stops dodging, so the run ends.
pub fn autopilot(w: &World, tired_at: f32) -> Control {
    let mut dir = V2::default();
    if let Some(e) = w.embers.iter().filter(|e| e.pos.y > TOP).min_by(|a, b| {
        let da = a.pos.minus(w.player).len();
        let db = b.pos.minus(w.player).len();
        da.total_cmp(&db)
    }) {
        dir = e.pos.minus(w.player).norm();
    }
    let tired = w.time > tired_at;
    let mut near = 0;
    for m in &w.moths {
        let away = w.player.minus(m.pos);
        let d = away.len();
        if d < w.radius() + 6.0 {
            near += 1;
        }
        if !tired && d < 34.0 {
            dir = dir.plus(away.norm().scale(2.2 * (34.0 - d) / 34.0));
        }
    }
    Control {
        dir: if dir.len() > 1.0 { dir.norm() } else { dir },
        flare: !tired && near >= 2 && w.light >= FLARE_COST + 10.0,
    }
}
