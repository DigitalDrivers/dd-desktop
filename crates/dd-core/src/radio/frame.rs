//! One sample of the physics and the graphics page, reduced to what the radio needs: the own car's pose and the
//! positions of the other cars by their ids. Offsets are those of the official header with `#pragma pack(4)` (research
//! 2026-11-09-boxenfunk/A-shared-memory-offsets.txt); the slots of `car_coordinates` and `car_ids` are read by id,
//! never by their index: nothing says the own car sits in slot 0.

use crate::evo_memory::{f32_at, GRAPHICS_SIZE};
use crate::laps::PHYSICS_SIZE;

/// The two halves of a car's id in `car_ids` and `player_car_id`; [`crate::evo_memory::car_id`] writes it as the
/// server's log does.
pub type CarId = (u64, u64);

// physics
const GAS: usize = 4;
const BRAKE: usize = 8;
const SPEED_KMH: usize = 28;
/// World frame, m/s
const VELOCITY: usize = 32;
const HEADING: usize = 208;
/// Pitch, yaw, roll: the yaw rate is the second
const LOCAL_ANGULAR_VEL: usize = 296;
/// FL FR RL RR, each x y z in the world frame of `car_coordinates`
const TYRE_CONTACT_POINT: usize = 420;
// graphics
const STATUS: usize = 4;
const PLAYER_CAR_ID: usize = 24;
const IS_IN_PIT_BOX: usize = 3119;
const IS_IN_PIT_LANE: usize = 3120;
const CAR_COORDINATES: usize = 3124;
const CAR_IDS: usize = 3940;
const SLOTS: usize = 60;
/// `ACEVO_STATUS`: 2 = live driving
const LIVE: i32 = 2;

/// One sample of the pages.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Seconds on a monotonic clock (a recording's `t_us` / 1e6)
    pub t: f64,
    /// The game drives live (not in a menu, a replay or the pause)
    pub live: bool,
    pub own: Own,
    /// Every slot with a car id that is not the own car's, with finite coordinates
    pub others: Vec<Car>,
}

/// The driver's own car.
#[derive(Debug, Clone, PartialEq)]
pub struct Own {
    pub id: CarId,
    /// `car_coordinates` of the slot that carries the player's id (graphics page, the same clock as the other cars)
    pub pos: [f32; 3],
    /// The wheels' contact points FL FR RL RR (physics page); None when all are zero
    pub contact: Option<[[f32; 3]; 4]>,
    /// World frame, m/s
    pub velocity: [f32; 3],
    /// The physics page's heading; its zero and sign are not known yet, so the spotter does not use it
    pub heading: f32,
    /// m/s, from `speedKmh`
    pub speed: f32,
    /// `localAngularVel[1]`, rad/s
    pub yaw_rate: f32,
    pub throttle: f32,
    pub brake: f32,
    pub in_pit_lane: bool,
    pub in_pit_box: bool,
}

/// Another car: only its position is published.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Car {
    pub id: CarId,
    pub pos: [f32; 3],
}

fn u64_at(page: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(page[at..at + 8].try_into().unwrap())
}

fn vec3_at(page: &[u8], at: usize) -> [f32; 3] {
    [f32_at(page, at), f32_at(page, at + 4), f32_at(page, at + 8)]
}

/// None for pages of another size, or when no slot carries the player's id (or its coordinates are not finite).
pub fn read(t: f64, physics: &[u8], graphics: &[u8]) -> Option<Frame> {
    if physics.len() != PHYSICS_SIZE || graphics.len() != GRAPHICS_SIZE {
        return None;
    }
    let player = (u64_at(graphics, PLAYER_CAR_ID), u64_at(graphics, PLAYER_CAR_ID + 8));
    let mut own_pos = None;
    let mut others = Vec::new();
    for slot in 0..SLOTS {
        let id = (u64_at(graphics, CAR_IDS + slot * 16), u64_at(graphics, CAR_IDS + slot * 16 + 8));
        if id == (0, 0) {
            continue;
        }
        let pos = vec3_at(graphics, CAR_COORDINATES + slot * 12);
        if !pos.iter().all(|v| v.is_finite()) {
            continue;
        }
        if id == player {
            own_pos = Some(pos);
        } else {
            others.push(Car { id, pos });
        }
    }
    let contact: [[f32; 3]; 4] = std::array::from_fn(|wheel| vec3_at(physics, TYRE_CONTACT_POINT + wheel * 12));
    Some(Frame {
        t,
        live: i32::from_le_bytes(graphics[STATUS..STATUS + 4].try_into().unwrap()) == LIVE,
        own: Own {
            id: player,
            pos: own_pos?,
            contact: contact.iter().flatten().any(|v| *v != 0.0).then_some(contact),
            velocity: vec3_at(physics, VELOCITY),
            heading: f32_at(physics, HEADING),
            speed: f32_at(physics, SPEED_KMH) / 3.6,
            yaw_rate: f32_at(physics, LOCAL_ANGULAR_VEL + 4),
            throttle: f32_at(physics, GAS),
            brake: f32_at(physics, BRAKE),
            in_pit_lane: graphics[IS_IN_PIT_LANE] != 0,
            in_pit_box: graphics[IS_IN_PIT_BOX] != 0,
        },
        others,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_f32s(page: &mut [u8], at: usize, values: &[f32]) {
        for (i, v) in values.iter().enumerate() {
            page[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn put_car(graphics: &mut [u8], slot: usize, id: CarId, pos: [f32; 3]) {
        graphics[CAR_IDS + slot * 16..CAR_IDS + slot * 16 + 8].copy_from_slice(&id.0.to_le_bytes());
        graphics[CAR_IDS + slot * 16 + 8..CAR_IDS + slot * 16 + 16].copy_from_slice(&id.1.to_le_bytes());
        put_f32s(graphics, CAR_COORDINATES + slot * 12, &pos);
    }

    const ME: CarId = (0x4f44a4be5be33c37, 0x2b051984457156ac);

    fn pages() -> (Vec<u8>, Vec<u8>) {
        let mut physics = vec![0u8; PHYSICS_SIZE];
        put_f32s(&mut physics, GAS, &[0.95, 0.0]);
        put_f32s(&mut physics, SPEED_KMH, &[144.0, 38.0, 0.5, -11.0]);
        put_f32s(&mut physics, HEADING, &[1.86]);
        put_f32s(&mut physics, LOCAL_ANGULAR_VEL, &[0.01, -0.2, 0.02]);
        put_f32s(&mut physics, TYRE_CONTACT_POINT, &[59.0, 5.0, 190.2, 59.5, 5.0, 191.8, 56.4, 5.0, 191.0, 56.9, 5.0, 192.6]);
        let mut graphics = vec![0u8; GRAPHICS_SIZE];
        graphics[STATUS..STATUS + 4].copy_from_slice(&LIVE.to_le_bytes());
        graphics[PLAYER_CAR_ID..PLAYER_CAR_ID + 8].copy_from_slice(&ME.0.to_le_bytes());
        graphics[PLAYER_CAR_ID + 8..PLAYER_CAR_ID + 16].copy_from_slice(&ME.1.to_le_bytes());
        put_car(&mut graphics, 0, (1, 2), [10.0, 5.0, -20.0]);
        // slot 1 stays empty; slot 2 is a car without finite coordinates
        put_car(&mut graphics, 2, (3, 4), [f32::NAN, 5.0, 1.0]);
        put_car(&mut graphics, 3, ME, [58.0, 5.0, 191.4]);
        put_car(&mut graphics, 59, (0, 9), [-1.5, 6.0, 2.5]);
        (physics, graphics)
    }

    #[test]
    fn reads_the_own_car_from_the_slot_with_the_player_s_id_and_the_others_by_id() {
        let (physics, graphics) = pages();
        let frame = read(12.5, &physics, &graphics).unwrap();
        assert_eq!(frame.t, 12.5);
        assert!(frame.live);
        assert_eq!(frame.own.id, ME);
        assert_eq!(frame.own.pos, [58.0, 5.0, 191.4], "slot 3, not slot 0");
        assert_eq!(frame.others, vec![Car { id: (1, 2), pos: [10.0, 5.0, -20.0] }, Car { id: (0, 9), pos: [-1.5, 6.0, 2.5] }], "no empty slot, no NaN, not the own car");
        let own = &frame.own;
        assert_eq!(own.contact.unwrap()[1], [59.5, 5.0, 191.8], "FR");
        assert_eq!(own.contact.unwrap()[3], [56.9, 5.0, 192.6], "RR");
        assert_eq!(own.velocity, [38.0, 0.5, -11.0]);
        assert!((own.speed - 40.0).abs() < 1e-5);
        assert_eq!((own.heading, own.yaw_rate, own.throttle, own.brake), (1.86, -0.2, 0.95, 0.0));
        assert_eq!((own.in_pit_lane, own.in_pit_box), (false, false));
    }

    #[test]
    fn reads_the_pit_flags_the_status_and_zero_contact_points() {
        let (mut physics, mut graphics) = pages();
        put_f32s(&mut physics, TYRE_CONTACT_POINT, &[0.0; 12]);
        graphics[IS_IN_PIT_LANE] = 1;
        graphics[IS_IN_PIT_BOX] = 1;
        graphics[STATUS] = 3;
        let frame = read(0.0, &physics, &graphics).unwrap();
        assert_eq!(frame.own.contact, None);
        assert_eq!((frame.own.in_pit_lane, frame.own.in_pit_box, frame.live), (true, true, false));
    }

    #[test]
    fn nothing_without_the_own_car_or_for_pages_of_another_size() {
        let (physics, mut graphics) = pages();
        assert!(read(0.0, &physics[..799], &graphics).is_none());
        assert!(read(0.0, &physics, &graphics[..4899]).is_none());
        put_f32s(&mut graphics, CAR_COORDINATES + 3 * 12, &[f32::INFINITY, 0.0, 0.0]);
        assert!(read(0.0, &physics, &graphics).is_none(), "the own car's coordinates are not finite");
        put_car(&mut graphics, 3, (0, 0), [0.0; 3]);
        assert!(read(0.0, &physics, &graphics).is_none(), "no slot carries the player's id");
    }
}
