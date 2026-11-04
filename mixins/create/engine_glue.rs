// Horizon's side of the steam engine: placement, its block entity's tick (the
// rules in boiler.rs), its pistons drawn, its rotation direction.

use super::Game;
use super::create_boiler::{self as rules, connected};
use super::create_fluids::SmartFace as Face;
use super::create_kinetics::{Axis, Dir, Kind, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_world::nbt::{JavaString, Tag};

pub(super) const ENGINE: &str = "create:steam_engine";
pub(super) const POWERED_SHAFT: &str = "create:powered_shaft";
const TANK: &str = "create:fluid_tank";

fn add(p: Pos, d: Dir, n: i32) -> Pos {
    let v = d.normal();
    [p[0] + v[0] * n, p[1] + v[1] * n, p[2] + v[2] * n]
}

fn state_at<'a>(game: &'a Game<'_>, pos: Pos) -> Option<&'a horizon_world::chunk::BlockState> {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))
}

/// The engine at `pos`: its `FACE` and `FACING`.
pub(super) fn engine_at(game: &Game<'_>, pos: Pos) -> Option<(Face, Dir)> {
    let s = state_at(game, pos).filter(|s| s.name == ENGINE)?;
    let face = s.properties.get("face").and_then(|f| Face::parse(f))?;
    Some((face, s.properties.get("facing").and_then(|f| Dir::parse(f))?))
}

/// `getConnectedDirection` of the engine at `pos`.
pub(super) fn engine_connected(game: &Game<'_>, pos: Pos) -> Option<Dir> {
    engine_at(game, pos).map(|(face, facing)| connected(face, facing))
}

/// `SteamEngineBlock.getStateForPlacement` with `canSurvive`: only on a tank.
pub(super) fn placement(
    level: &super::GameLevel<'_>,
    state: horizon_world::registry::BlockStateId,
    pos: Pos,
    clicked: Dir,
    yaw: f32,
    pitch: f32,
) -> Option<horizon_world::registry::BlockStateId> {
    let looks = super::create_placement::nearest_looking_directions(clicked, yaw, pitch);
    let horizontal = super::create_placement::horizontal(yaw);
    let (face, facing) = rules::placement(&looks, horizontal, |d| {
        let p = add(pos, d, 1);
        level
            .registry
            .state(level.world.block(p[0], p[1], p[2]))
            .is_some_and(|s| super::create_fluid_glue::is_tank_block(&s.name))
    })?;
    let s = level.native_state_with_property(state, "face", face.name())?;
    level.native_state_with_property(s, "facing", facing.name())
}

/// `SteamEngineBlockEntity`'s `movementDirection`: 0 clockwise.
fn rotation(entity: &ModBlockEntity) -> i32 {
    match entity.nbt.get(&JavaString::from("ScrollValue")) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}

/// `SteamEngineBlockEntity.tick`, with `SteamEngineBlock.onPlace`'s shaft made a
/// powered one (and `ShaftBlock.pickCorrectShaftType`'s, when placed after).
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some((face, facing)) = engine_at(game, pos) else {
        return;
    };
    let out = connected(face, facing);
    let shaft_pos = add(pos, out, 2);
    if let Some(Kind::Shaft(a)) = super::create_glue::kind_of(game, shaft_pos)
        && rules::shaft_valid(out, a)
    {
        super::create_glue::set_kind(game, shaft_pos, Kind::PoweredShaft(a));
    }
    let tank_pos = add(pos, out.opposite(), 1);
    let dimension = game.dim.id().to_owned();
    let shaft = super::create_glue::powered_shaft(&dimension, shaft_pos)
        .filter(|s| s.engine.is_none_or(|e| e == pos));
    let efficiency = super::create_fluid_glue::boiler_efficiency(&dimension, tank_pos);
    let valid = state_at(game, tank_pos).is_some_and(|s| s.name == TANK);
    let (Some(shaft), Some(efficiency), true) = (shaft, efficiency, valid) else {
        if let Some(s) = super::create_glue::powered_shaft(&dimension, shaft_pos)
            && s.engine == Some(pos)
            && s.efficiency != 0.0
            && game.world.loaded(tank_pos[0], tank_pos[2])
        {
            super::create_glue::update_engine(game, &dimension, shaft_pos, pos, 0, 0.0);
        }
        return;
    };
    let toward = if out.axis() == Axis::Y { facing } else { out };
    let efficiency = efficiency.clamp(0.0, 1.0);
    let source = shaft.has_source.then_some(shaft.speed);
    let old = rotation(entity);
    let (direction, rotation) = rules::conveyed_direction(efficiency, shaft.axis, toward, old, source);
    if rotation != old {
        entity.nbt.insert(JavaString::from("ScrollValue"), Tag::Int(rotation));
    }
    super::create_glue::update_engine(game, &dimension, shaft_pos, pos, direction, efficiency);
    // So clients know it is here (it holds nothing else to draw).
    super::create_machines::note_view(game, pos, vec![(String::new(), rotation as u32)]);
}

/// `PoweredShaftBlock.stillValid`.
pub(super) fn shaft_still_valid(game: &Game<'_>, pos: Pos, axis: Axis) -> bool {
    Dir::ALL.into_iter().filter(|d| d.axis() != axis).any(|d| {
        let engine = add(pos, d, 2);
        engine_connected(game, engine).is_some_and(|out| add(engine, out, 2) == pos && rules::shaft_valid(out, axis))
    })
}

/// `SteamEngineBlock.PlacementHelper`: a shaft in hand goes where the engine
/// wants it, on the first axis in the order the player looks that fits.
pub(super) fn click(game: &mut Game<'_>, pos: Pos, yaw: f32, pitch: f32) -> bool {
    if game.held().is_none_or(|s| &*s.item != "create:shaft") {
        return false;
    }
    let Some(out) = engine_connected(game, pos) else {
        return false;
    };
    let shaft_pos = add(pos, out, 2);
    let Some(axis) = super::create_placement::ordered_by_nearest(yaw, pitch)
        .into_iter()
        .map(Dir::axis)
        .find(|a| rules::shaft_valid(out, *a))
    else {
        return false;
    };
    let id = game.world.block(shaft_pos[0], shaft_pos[1], shaft_pos[2]);
    if !game.res.facts().get(id).replaceable {
        return true;
    }
    super::create_glue::set_kind(game, shaft_pos, Kind::PoweredShaft(axis));
    // BlockItem.place: one used up, but not in creative.
    if game.native_acting_player().map_or(0, |p| p.game_mode) != 1 {
        let hand = game.held_mut();
        if let Some(h) = hand.as_mut() {
            h.count -= 1;
            if h.count == 0 {
                *hand = None;
            }
        }
    }
    true
}

/// `IWrenchable.getRotatedBlockState`: `FACING` clockwise from above or below,
/// if it is still on a tank (`canSurvive`).
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos, face: Dir) -> Option<bool> {
    let (attach, facing) = engine_at(game, pos)?;
    if face.axis() != Axis::Y {
        return Some(true);
    }
    let turned = facing.clockwise(Axis::Y);
    let tank = add(pos, connected(attach, turned).opposite(), 1);
    if state_at(game, tank).is_some_and(|s| super::create_fluid_glue::is_tank_block(&s.name)) {
        game.native_set_block_property(pos, "facing", turned.name());
        let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
        super::create_glue::play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
    }
    Some(true)
}

/// `SteamEngineValueBox`: whether a hit at `point` on `side` of the block at
/// `pos` is on the engine's value box.
pub(super) fn hits_value_box(game: &Game<'_>, pos: Pos, side: Dir, point: [f64; 3]) -> bool {
    let Some((face, facing)) = engine_at(game, pos) else {
        return false;
    };
    let out = connected(face, facing);
    let Some(roll) = rules::value_box_roll(out, facing, side) else {
        return false;
    };
    let at = rules::value_box_offset(roll, out.horizontal_angle(), out.vertical_angle());
    let local = [0, 1, 2].map(|k| (point[k] - f64::from(pos[k])) as f32);
    (0..3).map(|k| (local[k] - at[k]).powi(2)).sum::<f32>().sqrt() < 0.4 / 2.0
}

/// `movementDirection.onlyActiveWhen`, then the next of its two values. The
/// line to tell the player.
pub(super) fn step_direction(game: &mut Game<'_>, pos: Pos) -> Option<&'static str> {
    let out = engine_connected(game, pos)?;
    let dimension = game.dim.id().to_owned();
    if super::create_glue::powered_shaft(&dimension, add(pos, out, 2)).is_some_and(|s| s.has_source) {
        return None;
    }
    let next = game.native_with_mod_block_entity(pos, |be| {
        let next = 1 - rotation(be);
        be.nbt.insert(JavaString::from("ScrollValue"), Tag::Int(next));
        next
    })?;
    Some(if next == 0 { "Rotation Direction: Clockwise" } else { "Rotation Direction: Counter-Clockwise" })
}

/// Where the shaft of the engine at `pos` is, for its goggles
/// (`addToGoggleTooltip`: the shaft's numbers).
pub(super) fn goggle_shaft(game: &Game<'_>, pos: Pos) -> Option<Pos> {
    engine_connected(game, pos).map(|out| add(pos, out, 2))
}

/// `SteamEngineRenderer`.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, time: f32, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some((face, facing)) = engine_at(game, pos) else {
            continue;
        };
        let corner = glam::DVec3::new(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        if corner.distance_squared(camera) > 128.0 * 128.0 {
            continue;
        }
        let toward = connected(face, facing);
        let shaft_pos = add(pos, toward, 2);
        let Some(view) = super::create_glue::client_view(shaft_pos).filter(|v| matches!(v.kind, Kind::PoweredShaft(_))) else {
            continue;
        };
        let axis = view.kind.rotation_axis();
        let degrees = super::create_kinetics::render_angle(view.kind, shaft_pos, view.speed, time);
        let Some(angle) = rules::target_angle(degrees.to_radians(), axis, toward) else {
            continue;
        };
        let roll90 = (toward.axis() != Axis::Y && axis == Axis::Y) || (toward.axis() == Axis::Y && axis == Axis::Z);
        let (piston, turn) = rules::piston_motion(angle);
        let centre = glam::Vec3::splat(0.5);
        let base = glam::Mat4::from_translation((corner - camera).as_vec3())
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y(toward.horizontal_angle().to_radians())
            * glam::Mat4::from_rotation_x((toward.vertical_angle() + 90.0).to_radians())
            * glam::Mat4::from_rotation_y(if roll90 { -90f32.to_radians() } else { 0.0 })
            * glam::Mat4::from_translation(-centre);
        let lift = glam::Mat4::from_translation(glam::Vec3::new(0.0, piston + 20.0 / 16.0, 0.0));
        let pin = glam::Vec3::new(0.0, 4.0 / 16.0, 8.0 / 16.0);
        let parts = [
            ("create:hz_part_engine_piston", base * lift),
            (
                "create:hz_part_engine_linkage",
                base * glam::Mat4::from_translation(glam::Vec3::Y)
                    * lift
                    * glam::Mat4::from_translation(pin)
                    * glam::Mat4::from_rotation_x(turn)
                    * glam::Mat4::from_translation(-pin),
            ),
            (
                "create:hz_part_engine_connector",
                base * glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0, 0.0))
                    * glam::Mat4::from_translation(centre)
                    * glam::Mat4::from_rotation_x(-(angle + std::f32::consts::FRAC_PI_2))
                    * glam::Mat4::from_translation(-centre),
            ),
        ];
        let light = game.world.light(pos[0], pos[1], pos[2]);
        let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
        let seed = horizon_core::random::position_seed(horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]));
        for (holder, m) in parts {
            if let Some(state) = game.pack.assets.registry.default_state(holder) {
                game.block_model_vertices(state, seed, m, packed, false, out);
            }
        }
    }
}
