// Horizon's side of the smart observer and the threshold switch (rules in
// detector.rs): their block entities' ticks, what they read at their target, and
// the switch's screen. The switch's `powered` property is `poweredAfterDelay`.

use super::Game;
use super::create_detector::{self as rules, Config, Inputs, Observer, Switch, Target, TargetType};
use super::create_fluids::{FluidStack, PipeFilter};
use super::create_kinetics::{Dir, Pos};
use horizon_client::containers::ModBlockEntity;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::sync::Mutex;

pub(super) const OBSERVER: &str = "create:content_observer";
pub(super) const SWITCH: &str = "create:stockpile_switch";
const MOD_ID: &str = "create";

pub(super) fn is_ours(name: &str) -> bool {
    name == OBSERVER || name == SWITCH
}

fn add(pos: Pos, d: Dir) -> Pos {
    let n = d.normal();
    [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]]
}

fn block_id(game: &Game<'_>, pos: Pos) -> Option<String> {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).map(|s| s.name.clone())
}

/// The block's `target`, `facing`, if it is one of these.
fn aim_at(game: &Game<'_>, pos: Pos) -> Option<(&'static str, Target, Dir)> {
    let s = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2]))?;
    let name = if s.name == OBSERVER {
        OBSERVER
    } else if s.name == SWITCH {
        SWITCH
    } else {
        return None;
    };
    let target = s.properties.get("target").and_then(|v| Target::parse(v))?;
    let facing = s.properties.get("facing").and_then(|v| Dir::parse(v))?;
    Some((name, target, facing))
}

fn int(be: &ModBlockEntity, key: &str, default: i32) -> i32 {
    match be.nbt.get(&JavaString::from(key)) {
        Some(Tag::Int(v)) => *v,
        _ => default,
    }
}

fn set_int(be: &mut ModBlockEntity, key: &str, v: i32) {
    be.nbt.insert(JavaString::from(key), Tag::Int(v));
}

fn filter_of(be: &ModBlockEntity) -> Option<String> {
    match be.nbt.get(&JavaString::from("Filter")) {
        Some(Tag::String(t)) => Some(t.to_string_lossy()).filter(|t| !t.is_empty()),
        _ => None,
    }
}

fn passes(filter: Option<&str>, item: &str) -> bool {
    filter.is_none_or(|f| f == item)
}

/// `FilteringBehaviour.test` on a fluid.
fn passes_fluid(filter: Option<&str>, fluid: &FluidStack) -> bool {
    PipeFilter::of(filter).passes(fluid)
}

// ---- The smart observer ---------------------------------------------------------

fn load_observer(be: &ModBlockEntity) -> Observer {
    Observer { turn_off: int(be, "TurnOff", 0), off_due: int(be, "OffDue", 0) != 0 }
}

fn save_observer(be: &mut ModBlockEntity, o: Observer) {
    set_int(be, "TurnOff", o.turn_off);
    set_int(be, "OffDue", i32::from(o.off_due));
}

fn set_powered(game: &mut Game<'_>, pos: Pos, on: bool) {
    game.native_set_block_property(pos, "powered", if on { "true" } else { "false" });
    let dimension = game.dim.id().to_owned();
    game.native_update_signal_neighbors(&dimension, pos);
}

fn is_powered(game: &Game<'_>, pos: Pos) -> bool {
    game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).is_some_and(|s| s.properties.get("powered").is_some_and(|v| v == "true"))
}

/// `SmartObserverBlockEntity.tick`'s look at the target: how long to stay on.
fn observe(game: &mut Game<'_>, target: Pos, filter: Option<&str>, turn_off: i32) -> Option<i32> {
    if filter.is_some()
        && let Some(block) = block_id(game, target)
        && passes(filter, &block)
    {
        return Some(rules::BLOCK_DELAY);
    }
    // TransportedItemStackHandlerBehaviour: belts and depots, items near the
    // middle.
    if let Some(items) = super::create_logistics::centred_items(game, target, 0.45) {
        let any = turn_off != rules::DEFAULT_DELAY && items.iter().any(|s| passes(filter, &s.item));
        return any.then_some(rules::DEFAULT_DELAY);
    }
    // FluidTransportBehaviour.
    if let Some(flows) = super::create_fluid_glue::inbound_flows(game, target) {
        return flows.iter().any(|f| passes_fluid(filter, f)).then_some(rules::DEFAULT_DELAY);
    }
    if super::create_logistics::any_extractable(game, target, &|s: &ItemStack| passes(filter, &s.item)) {
        return Some(rules::DEFAULT_DELAY);
    }
    let tanks = super::create_fluid_glue::observed_tanks(game, target).unwrap_or_default();
    tanks.iter().any(|(f, _)| !f.is_empty() && passes_fluid(filter, f)).then_some(rules::DEFAULT_DELAY)
}

fn observer_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some((_, target, facing)) = aim_at(game, pos) else {
        return;
    };
    let mut o = load_observer(entity);
    if o.tick() {
        set_powered(game, pos, false);
    }
    let filter = filter_of(entity);
    let at = add(pos, rules::target_direction(target, facing));
    if let Some(ticks) = observe(game, at, filter.as_deref(), o.turn_off) {
        o.activate(ticks);
        if !is_powered(game, pos) {
            set_powered(game, pos, true);
        }
    }
    save_observer(entity, o);
    super::create_machines::note_view(game, pos, vec![(filter.unwrap_or_default(), 1)]);
}

/// `SmartObserverBlock.onFunnelTransfer`: observers watching this funnel.
pub(super) fn funnel_transfer(game: &mut Game<'_>, funnel: Pos, item: &str) {
    for d in Dir::ALL {
        let at = add(funnel, d);
        let Some((OBSERVER, target, facing)) = aim_at(game, at) else {
            continue;
        };
        if rules::target_direction(target, facing) != d.opposite() {
            continue;
        }
        let took = game.native_with_mod_block_entity(at, |be| {
            let ok = passes(filter_of(be).as_deref(), item);
            if ok {
                set_int(be, "TurnOff", rules::FUNNEL_DELAY);
            }
            ok
        });
        if took == Some(true) && !is_powered(game, at) {
            set_powered(game, at, true);
        }
    }
}

// ---- The threshold switch -------------------------------------------------------

fn load_switch(be: &ModBlockEntity) -> Switch {
    let d = Switch::default();
    let flag = |k: &str| int(be, k, 0) != 0;
    Switch {
        on_above: int(be, "OnAboveAmount", d.on_above),
        off_below: int(be, "OffBelowAmount", d.off_below),
        min: int(be, "CurrentMinAmount", d.min),
        level: int(be, "CurrentAmount", d.level),
        max: int(be, "CurrentMaxAmount", d.max),
        in_stacks: flag("InStacks"),
        redstone: flag("Powered"),
        inverted: flag("Inverted"),
        powered_after_delay: flag("PoweredAfterDelay"),
    }
}

fn save_switch(be: &mut ModBlockEntity, s: &Switch) {
    set_int(be, "OnAboveAmount", s.on_above);
    set_int(be, "OffBelowAmount", s.off_below);
    set_int(be, "CurrentMinAmount", s.min);
    set_int(be, "CurrentAmount", s.level);
    set_int(be, "CurrentMaxAmount", s.max);
    set_int(be, "InStacks", i32::from(s.in_stacks));
    set_int(be, "Powered", i32::from(s.redstone));
    set_int(be, "Inverted", i32::from(s.inverted));
    set_int(be, "PoweredAfterDelay", i32::from(s.powered_after_delay));
}

/// `isSuitableInventory`: not what holds a `ProcessingInventory`.
fn suitable_inventory(game: &Game<'_>, at: Pos) -> bool {
    block_id(game, at).is_none_or(|n| !super::create_machines::is_saw(&n) && n != super::create_crusher_glue::CONTROLLER)
}

/// What the switch reads at `at`: `updateCurrentLevel`'s levels and
/// `getTypeOfCurrentTarget`.
fn read_target(game: &mut Game<'_>, at: Pos, filter: Option<&str>) -> (Option<(i32, i32, i32)>, TargetType) {
    // ThresholdSwitchObservable: the rope pulley.
    if let Some(offset) = super::create_contraptions::rope_length(game, at) {
        let v = (game.world.shape.min_y, at[1] - offset as i32, at[1]);
        return (Some(v), TargetType::Custom);
    }
    let stacks = if suitable_inventory(game, at) { super::create_logistics::observed_stacks(game, at) } else { None };
    let tanks = super::create_fluid_glue::observed_tanks(game, at);
    if stacks.is_none() && tanks.is_none() {
        return (None, TargetType::Unsupported);
    }
    let kind = if stacks.is_some() { TargetType::Item } else { TargetType::Fluid };
    let (mut level, mut max) = (0i64, 0i64);
    for (stack, limit) in stacks.iter().flatten() {
        let space = stack.as_ref().map_or(64, |s| super::create_machines::max_stack(game, s)).min(*limit);
        if space == 0 {
            continue;
        }
        max += i64::from(space);
        if let Some(s) = stack
            && passes(filter, &s.item)
        {
            level += i64::from(s.count);
        }
    }
    for (fluid, space) in tanks.iter().flatten() {
        if *space == 0 {
            continue;
        }
        max += i64::from(*space);
        if passes_fluid(filter, fluid) {
            level += i64::from(fluid.amount);
        }
    }
    let clamp = |v: i64| v.min(i64::from(i32::MAX)) as i32;
    (Some((0, clamp(level), clamp(max))), kind)
}

/// `updatePowerAfterDelay`.
fn update_power(game: &mut Game<'_>, pos: Pos, s: &mut Switch) {
    s.powered_after_delay = s.should_be_powered();
    if is_powered(game, pos) != s.powered_after_delay {
        set_powered(game, pos, s.powered_after_delay);
    }
}

fn switch_tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let Some((_, target, facing)) = aim_at(game, pos) else {
        return;
    };
    let mut s = load_switch(entity);
    // The block tick `scheduleBlockTick` asks for.
    let delay = int(entity, "PowerDelay", 0);
    if delay > 0 {
        set_int(entity, "PowerDelay", delay - 1);
        if delay == 1 {
            update_power(game, pos, &mut s);
        }
    }
    let at = add(pos, rules::target_direction(target, facing));
    let filter = filter_of(entity);
    // lazyTick, every ten (and onPlace, the first).
    let lazy = int(entity, "LazyTick", 0);
    if lazy <= 0 {
        set_int(entity, "LazyTick", 10);
        let (observed, kind) = read_target(game, at, filter.as_deref());
        set_int(entity, "TargetType", kind.index() as i32);
        let update = s.update(observed);
        if let Some(display) = update.display {
            let text = display.to_string();
            let shown = game.pack.assets.registry.state(game.world.block(pos[0], pos[1], pos[2])).and_then(|b| b.properties.get("level").cloned());
            if shown.as_deref() != Some(text.as_str()) {
                game.native_set_block_property(pos, "level", &text);
            }
        }
        if update.schedule && int(entity, "PowerDelay", 0) == 0 {
            set_int(entity, "PowerDelay", 2);
        }
    } else {
        set_int(entity, "LazyTick", lazy - 1);
    }
    save_switch(entity, &s);
    // Clients read it for the screen, in bytes.
    let kind = int(entity, "TargetType", 0) as u32;
    let flags = kind | u32::from(s.in_stacks) << 2 | u32::from(s.inverted) << 3 | u32::from(s.powered_after_delay) << 4;
    let mut view = vec![(filter.unwrap_or_default(), 1), (String::new(), flags)];
    for v in [s.level, s.min, s.max, s.on_above, s.off_below] {
        view.extend(v.to_le_bytes().map(|b| (String::new(), u32::from(b))));
    }
    super::create_machines::note_view(game, pos, view);
}

/// `native_block_entity_tick`.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    if entity.id == OBSERVER {
        observer_tick(game, pos, entity);
    } else {
        switch_tick(game, pos, entity);
    }
}

/// `ConfigureThresholdSwitchPacket`, a field at a time.
pub(super) fn configure(game: &mut Game<'_>, pos: Pos, value: i32) {
    let Some(config) = Config::decode(value) else {
        return;
    };
    let flipped = game.native_with_mod_block_entity(pos, |be| {
        let mut s = load_switch(be);
        let mut flipped = false;
        match config {
            Config::OffBelow(n) => s.off_below = n,
            Config::OnAbove(n) => s.on_above = n,
            Config::Flags { inverted, stacks } => {
                flipped = s.inverted != inverted;
                s.inverted = inverted;
                s.in_stacks = stacks;
            }
        }
        if flipped {
            s.powered_after_delay = s.should_be_powered();
        }
        save_switch(be, &s);
        flipped.then_some(s.powered_after_delay)
    });
    if let Some(Some(on)) = flipped {
        set_powered(game, pos, on);
    }
}

// ---- Both: the block --------------------------------------------------------------

/// Whether something at `at` can be detected (`getStateForPlacement`):
/// capabilities on both, and on the observer belts, depots, pipes, funnels.
fn detectable(level: &super::GameLevel<'_>, at: Pos, observer: bool) -> bool {
    use horizon_client::containers::BlockEntity as B;
    let Some(s) = level.registry.state(level.world.block(at[0], at[1], at[2])) else {
        return false;
    };
    let name = s.name.as_str();
    if super::create_logistics::is_belt(name) || super::create_fluid_glue::is_tank_block(name) || super::create_logistics::is_chute_block(name) {
        return true;
    }
    if observer && (super::create_funnel::is_funnel(name) || super::create_fluid_glue::is_pipe_block_name(name)) {
        return true;
    }
    match level.world.block_entities.get(&(at[0], at[1], at[2])) {
        Some(B::Chest(_) | B::Furnace(_) | B::Brewing(_) | B::Crafter(_)) => true,
        Some(B::Mod(m)) => {
            let mut c = None;
            super::create_machines::block_entity_container(&m.id, &mut c);
            c.is_some()
                || [
                    super::create_machines::DEPOT,
                    super::create_machines::BASIN,
                    super::create_ejector_glue::EJECTOR,
                    super::create_spout_glue::SPOUT,
                    super::create_spout_glue::DRAIN,
                    super::create_hose_glue::BLOCK,
                    super::create_arms::VAULT,
                ]
                .contains(&m.id.as_str())
        }
        _ => false,
    }
}

/// `getStateForPlacement`.
pub(super) fn placement(
    level: &super::GameLevel<'_>,
    state: BlockStateId,
    pos: Pos,
    clicked: Dir,
    yaw: f32,
    pitch: f32,
    sneaking: bool,
) -> Option<BlockStateId> {
    let observer = level.registry.state(state)?.name == OBSERVER;
    let looks = super::create_placement::nearest_looking_directions(clicked, yaw, pitch);
    let horizontal = super::create_placement::horizontal(yaw);
    let look = super::create_placement::nearest_looking(yaw, pitch);
    let (target, facing) = rules::placement(&looks, |d| detectable(level, add(pos, d), observer), look, sneaking, horizontal);
    let s = level.native_state_with_property(state, "target", target.name())?;
    level.native_state_with_property(s, "facing", facing.name())
}

/// `native_block_signal`: 15 when on, except towards its target; none direct.
pub(super) fn block_signal(level: &super::GameLevel<'_>, state: BlockStateId, face: Dir, direct: bool, signal: &mut i32) {
    let Some(s) = level.registry.state(state) else {
        return;
    };
    let target = s.properties.get("target").and_then(|v| Target::parse(v));
    let facing = s.properties.get("facing").and_then(|v| Dir::parse(v));
    let on = s.properties.get("powered").is_some_and(|v| v == "true");
    let (Some(target), Some(facing)) = (target, facing) else {
        return;
    };
    *signal = if on && !direct && face != rules::target_direction(target, facing) { 15 } else { 0 };
}

/// `onWrenched` by `getRotatedBlockState`.
pub(super) fn wrench(game: &mut Game<'_>, pos: Pos, face: Dir) -> Option<bool> {
    let (_, target, facing) = aim_at(game, pos)?;
    let (next_target, next_facing) = rules::wrenched(target, facing, face);
    if (next_target, next_facing) != (target, facing) {
        game.native_set_block_property(pos, "target", next_target.name());
        game.native_set_block_property(pos, "facing", next_facing.name());
        let pitch = horizon_core::random::RandomSource::next_float(&mut game.world.random) + 0.5;
        super::create_glue::play_create_sound(game, "wrench_rotate", pos.map(|v| f64::from(v) + 0.5), 1.0, pitch);
    }
    Some(true)
}

/// `FilteringBehaviour` on the slot of the clicked side, then the block's own:
/// the switch's screen. True when it took the click.
pub(super) fn right_click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let Some((name, target, facing)) = aim_at(game, pos) else {
        return false;
    };
    let held = game.held().map(|s| s.item.to_string());
    if held.as_deref() == Some(super::create_glue::WRENCH) {
        return false;
    }
    let side = Dir::nearest(face);
    if held.as_deref() != Some(super::create_arms::ARM) && rules::side_active(target, facing, side, name == SWITCH) {
        let at = rules::slot_centre(side);
        let hit = game.pick().filter(|h| h.pos == pos).map(|h| [0, 1, 2].map(|k| (h.point[k] - f64::from(pos[k])) as f32));
        // ValueBoxTransform.testHit: within half its scale, 0.4.
        if hit.is_some_and(|h| (0..3).map(|k| (h[k] - at[k]).powi(2)).sum::<f32>().sqrt() < 0.2) {
            let text = held.unwrap_or_default();
            game.native_with_mod_block_entity(pos, |be| {
                be.nbt.insert(JavaString::from("Filter"), Tag::String(text.as_str().into()));
                // withCallback: the switch reads again.
                set_int(be, "LazyTick", 0);
            });
            return true;
        }
    }
    if name != SWITCH {
        return false;
    }
    if let Some(player) = game.native_acting_player() {
        let mut message = b"T1".to_vec();
        for v in pos {
            message.extend_from_slice(&v.to_le_bytes());
        }
        if let Err(e) = game.native_net_send_to_player(player.uuid, super::create_sequencer_glue::CHANNEL, &message) {
            log::debug!("Create: threshold switch screen: {e}");
        }
    }
    true
}

/// `FilteringRenderer`: the filter on each active side.
pub(super) fn draw(game: &Game<'_>, camera: glam::DVec3, out: &mut Vec<horizon_client::mesher::Vertex>) {
    for pos in super::create_machines::client_positions() {
        let Some((name, target, facing)) = aim_at(game, pos) else {
            continue;
        };
        let Some(id) = super::create_machines::client_slots(pos).and_then(|s| s.first().map(|v| v.0.clone())).filter(|i| !i.is_empty()) else {
            continue;
        };
        for side in Dir::ALL {
            if rules::side_active(target, facing, side, name == SWITCH) {
                let turns = rules::slot_turns(side, facing);
                super::create_logistics::draw_filter_item_turned(game, camera, pos, &id, rules::slot_centre(side), turns, name, out);
            }
        }
    }
}

// ---- The switch's screen ----------------------------------------------------------

/// `ThresholdSwitchScreen`, as this client edits it.
struct Screen {
    pos: Pos,
    inputs: Inputs,
}

static SCREEN: Mutex<Option<Screen>> = Mutex::new(None);
static OPENING: Mutex<Option<Pos>> = Mutex::new(None);
static OUTBOX: Mutex<Outbox> = Mutex::new(Outbox { close: false, flip: false, sound: None });

struct Outbox {
    close: bool,
    flip: bool,
    sound: Option<f32>,
}

fn screen() -> std::sync::MutexGuard<'static, Option<Screen>> {
    SCREEN.lock().unwrap_or_else(|e| e.into_inner())
}

fn outbox() -> std::sync::MutexGuard<'static, Outbox> {
    OUTBOX.lock().unwrap_or_else(|e| e.into_inner())
}

/// `create:screens` on a client.
pub(super) fn from_server(data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"T1") else {
        return;
    };
    if rest.len() != 12 {
        return;
    }
    let int = |i: usize| i32::from_le_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
    *OPENING.lock().unwrap_or_else(|e| e.into_inner()) = Some([int(0), int(4), int(8)]);
}

/// The block entity as clients know it.
struct View {
    kind: TargetType,
    switch: Switch,
}

fn view_at(pos: Pos) -> Option<View> {
    let slots = super::create_machines::client_slots(pos)?;
    let flags = slots.get(1)?.1;
    let read = |i: usize| -> Option<i32> {
        let b = slots.get(2 + i * 4..2 + i * 4 + 4)?;
        Some(i32::from_le_bytes([b[0].1 as u8, b[1].1 as u8, b[2].1 as u8, b[3].1 as u8]))
    };
    let switch = Switch {
        level: read(0)?,
        min: read(1)?,
        max: read(2)?,
        on_above: read(3)?,
        off_below: read(4)?,
        in_stacks: flags & 4 != 0,
        redstone: false,
        inverted: flags & 8 != 0,
        powered_after_delay: flags & 16 != 0,
    };
    Some(View { kind: TargetType::from_index(flags & 3), switch })
}

const WIDTH: f32 = 182.0;
const HEIGHT: f32 = 103.0;

fn corner(rect: [f32; 4]) -> [f32; 2] {
    // setWindowSize, setWindowOffset(-20, 0).
    [rect[0] + (rect[2] - WIDTH) / 2.0 - 20.0, rect[1] + (rect[3] - HEIGHT) / 2.0]
}

fn over(mouse: [f32; 2], x: f32, y: f32, w: f32, h: f32) -> bool {
    (x..x + w).contains(&mouse[0]) && (y..y + h).contains(&mouse[1])
}

#[derive(Clone, Copy, PartialEq)]
enum Widget {
    On,
    Off,
    Measure,
}

/// `updateInputBoxes`' widths and visibility: the input under the mouse.
fn widget_at(kind: TargetType, at: [f32; 2], mouse: [f32; 2]) -> Option<Widget> {
    let [x, y] = at;
    let items = kind == TargetType::Item;
    if items && over(mouse, x + 100.0, y + 23.0, 52.0, 42.0) {
        return Some(Widget::Measure);
    }
    if kind == TargetType::Unsupported {
        return None;
    }
    let w = if items { 48.0 } else { 103.0 };
    if over(mouse, x + 48.0, y + 23.0, w, 18.0) {
        return Some(Widget::On);
    }
    over(mouse, x + 48.0, y + 47.0, w, 18.0).then_some(Widget::Off)
}

/// `getHoverName`, for a block's item.
fn item_name(game: &Game<'_>, id: &str) -> String {
    game.item_display_name(id).plain()
}

fn tooltip(game: &Game<'_>, batch: &mut horizon_client::hud::HudBatch, mouse: [f32; 2], lines: &[(String, u32)]) {
    let width = lines.iter().map(|l| game.native_text_width(&l.0)).fold(0.0, f32::max) + 8.0;
    let (tx, ty) = (mouse[0] + 10.0, mouse[1] - 8.0);
    game.native_fill(batch, [tx, ty, width, lines.len() as f32 * 10.0 + 6.0], 0xF0_10_00_18);
    for (i, (line, rgb)) in lines.iter().enumerate() {
        game.native_text(batch, line, [tx + 4.0, ty + 3.0 + i as f32 * 10.0], *rgb, true);
    }
}

const HEADER: u32 = 0x53_91_E1;
const GRAY: u32 = 0xAA_AA_AA;
const DARK_GRAY: u32 = 0x55_55_55;
const AQUA: u32 = 0x00_AA_AA;
const WHITE: u32 = 0xFF_FF_FF;

/// `native_screen_render_post`: `renderWindow`.
pub(super) fn render(game: &Game<'_>, batch: &mut horizon_client::hud::HudBatch, rect: [f32; 4], mouse: [f32; 2]) {
    if game.native_mod_screen() != Some(MOD_ID) {
        return;
    }
    let guard = screen();
    let Some(s) = guard.as_ref() else {
        return;
    };
    let Some(view) = view_at(s.pos) else {
        return;
    };
    let at = corner(rect);
    let [x, y] = at;
    let sheet = "create:gui/threshold_switch";
    game.native_sprite(batch, sheet, [0.0, 0.0, WIDTH, HEIGHT], [x, y, WIDTH, HEIGHT]);
    let title = "Threshold Switch";
    let centre = x + (WIDTH / 2.0).floor() - (game.native_text_width(title) / 2.0).floor();
    game.native_text(batch, title, [centre, y + 4.0], 0x59_24_24, false);
    let kind = view.kind;
    let items = kind == TargetType::Item;
    // THRESHOLD_SWITCH_ITEMCOUNT_INPUTS, THRESHOLD_SWITCH_MISC_INPUTS.
    let src = if items { [0.0, 105.0, 115.0, 22.0] } else { [0.0, 129.0, 115.0, 22.0] };
    for row in [0.0, 24.0] {
        game.native_sprite(batch, sheet, src, [x + 44.0, y + 21.0 + row, 115.0, 22.0]);
    }
    let stacks = s.inputs.stacks;
    let step = rules::value_step(kind, stacks);
    if items {
        let suffix = if stacks { "Stacks" } else { "Items" };
        game.native_text(batch, suffix, [x + 105.0, y + 28.0], WHITE, true);
        game.native_text(batch, suffix, [x + 105.0, y + 52.0], WHITE, true);
    }
    let shown = |v: i32| match kind {
        TargetType::Unsupported => String::new(),
        TargetType::Item => (v / step).to_string(),
        _ => rules::format_value(kind, stacks, v / step),
    };
    game.native_text(batch, &format!("\u{2265} {}", shown(s.inputs.on.state)), [x + 53.0, y + 28.0], WHITE, true);
    game.native_text(batch, &format!("\u{2264} {}", shown(s.inputs.off.state)), [x + 53.0, y + 52.0], WHITE, true);
    // The block, five times its size.
    game.native_item(batch, SWITCH, [x + WIDTH + 6.0, y + HEIGHT - 56.0], 80.0);
    // What it is attached to.
    let (item_x, item_y) = (x + 13.0, y + 80.0);
    let target = aim_at(game, s.pos).map(|(_, t, f)| add(s.pos, rules::target_direction(t, f)));
    let attached = target.and_then(|t| block_id(game, t)).filter(|n| n != "minecraft:air");
    let drawn = attached.as_deref().is_some_and(|n| game.native_item(batch, n, [item_x, item_y], 16.0));
    if !drawn {
        game.native_item(batch, "minecraft:barrier", [item_x, item_y], 16.0);
    }
    // THRESHOLD_SWITCH_CURRENT_STATE on the row it is in, and the two torches.
    let (torch_x, torch_y) = (x + 23.0, y + 24.0);
    let powered = view.switch.powered_after_delay;
    let inverted = view.switch.inverted;
    let top = inverted != powered;
    game.native_sprite(batch, sheet, [0.0, 153.0, 24.0, 24.0], [torch_x - 3.0, torch_y - 4.0 + if top { 0.0 } else { 24.0 }, 24.0, 24.0]);
    for (row, lit) in [(0.0, !inverted), (26.0, inverted)] {
        let at = [torch_x, torch_y + row];
        if lit {
            game.native_item(batch, "minecraft:redstone_torch", at, 16.0);
        } else {
            game.native_item_model(batch, "minecraft:redstone_torch", "create:hz_redstone_torch_off", at, 16.0);
        }
    }
    // The buttons.
    let (cx, by) = (x + WIDTH - 33.0, y + HEIGHT - 24.0);
    let fx = x + WIDTH - 62.0;
    let over_confirm = over(mouse, cx, by, 18.0, 18.0);
    let over_flip = over(mouse, fx, by, 18.0, 18.0);
    for (bx, hover, icon) in [(cx, over_confirm, [0.0, 16.0]), (fx, over_flip, [128.0, 64.0])] {
        game.native_sprite(batch, "create:gui/widgets", [if hover { 18.0 } else { 0.0 }, 0.0, 18.0, 18.0], [bx, by, 18.0, 18.0]);
        game.native_sprite(batch, "create:gui/icons", [icon[0], icon[1], 16.0, 16.0], [bx + 1.0, by + 1.0, 16.0, 16.0]);
    }
    // Tooltips.
    if over(mouse, item_x, item_y, 16.0, 16.0) {
        let mut lines: Vec<(String, u32)> = Vec::new();
        if !drawn {
            lines.push(("Not attached to a block".to_owned(), WHITE));
        } else {
            let name = attached.as_deref().unwrap_or_default();
            lines.push((item_name(game, name), WHITE));
            if kind == TargetType::Unsupported {
                lines.push(("Incompatible block".to_owned(), GRAY));
            } else {
                let v = &view.switch;
                let f = |n: i32| rules::format_value(kind, stacks, n / step);
                lines.push((format!("Currently {}", f(v.level)), AQUA));
                if v.min / step == 0 {
                    lines.push((format!("(max. {})", f(v.max)), GRAY));
                } else {
                    lines.push((format!("({} to {})", v.min / step, f(v.max)), GRAY));
                }
            }
        }
        tooltip(game, batch, mouse, &lines);
        return;
    }
    for (row, power) in [(0.0, true), (26.0, false)] {
        if over(mouse, torch_x, torch_y + row, 16.0, 16.0) {
            let line = if power != inverted { "Power on when..." } else { "Power off when..." };
            tooltip(game, batch, mouse, &[(line.to_owned(), HEADER)]);
            return;
        }
    }
    if over_flip {
        tooltip(game, batch, mouse, &[("Invert Signal".to_owned(), WHITE)]);
        return;
    }
    let lines: Option<Vec<(String, u32)>> = match widget_at(kind, at, mouse) {
        Some(Widget::Measure) => Some(vec![
            ("Item Measure".to_owned(), HEADER),
            (format!("{} Items", if stacks { "  " } else { "->" }), WHITE),
            (format!("{} Stacks", if stacks { "->" } else { "  " }), WHITE),
            ("Scroll to Select".to_owned(), DARK_GRAY),
        ]),
        Some(Widget::On) => Some(vec![("...at or above:".to_owned(), HEADER), ("Scroll to Modify".to_owned(), DARK_GRAY)]),
        Some(Widget::Off) => Some(vec![("...at or below:".to_owned(), HEADER), ("Scroll to Modify".to_owned(), DARK_GRAY)]),
        None => None,
    };
    if let Some(lines) = lines {
        tooltip(game, batch, mouse, &lines);
    }
}

/// `native_screen_mouse_scrolled_pre`.
pub(super) fn scrolled(game: &Game<'_>, rect: [f32; 4], mouse: [f32; 2], up: bool, shift: bool) -> bool {
    if game.native_mod_screen() != Some(MOD_ID) {
        return false;
    }
    let mut guard = screen();
    let Some(s) = guard.as_mut() else {
        return false;
    };
    let Some(view) = view_at(s.pos) else {
        return true;
    };
    let kind = view.kind;
    let pitch = match widget_at(kind, corner(rect), mouse) {
        Some(Widget::On) => s.inputs.scroll_on(kind, up, shift),
        Some(Widget::Off) => s.inputs.scroll_off(kind, up, shift),
        Some(Widget::Measure) => s.inputs.scroll_stacks(up),
        None => None,
    };
    if let Some(pitch) = pitch {
        outbox().sound = Some(pitch);
    }
    true
}

/// `native_screen_mouse_button_pressed_pre`.
pub(super) fn pressed(game: &Game<'_>, rect: [f32; 4], mouse: [f32; 2], button: i32) -> bool {
    if game.native_mod_screen() != Some(MOD_ID) || screen().is_none() {
        return false;
    }
    if button == 0 {
        let [x, y] = corner(rect);
        let by = y + HEIGHT - 24.0;
        if over(mouse, x + WIDTH - 33.0, by, 18.0, 18.0) {
            outbox().close = true;
        } else if over(mouse, x + WIDTH - 62.0, by, 18.0, 18.0) {
            outbox().flip = true;
        }
    }
    true
}

/// `send`.
fn send(game: &mut Game<'_>, pos: Pos, inputs: &Inputs, invert: bool) {
    for config in [
        Config::OffBelow(inputs.off.state),
        Config::OnAbove(inputs.on.state),
        Config::Flags { inverted: invert, stacks: inputs.stacks },
    ] {
        super::create_glue::send_value(game, pos, config.encode());
    }
}

/// Once a tick on the window's client.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    let opening = OPENING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(pos) = opening
        && let Some(view) = view_at(pos)
        && game.native_mod_screen_open(MOD_ID)
    {
        *screen() = Some(Screen { pos, inputs: Inputs::new(&view.switch, view.kind) });
    }
    let (close, flip, sound) = {
        let mut o = outbox();
        (std::mem::take(&mut o.close), std::mem::take(&mut o.flip), o.sound.take())
    };
    if close {
        game.native_mod_screen_close(MOD_ID);
    }
    if let Some(pitch) = sound
        && let Some(p) = game.player.as_ref()
    {
        let at = [p.position.x, p.position.y, p.position.z];
        game.native_play_local_sound("create:scroll_value", at, 0.124, pitch, 0);
    }
    let Some((pos, was_open)) = screen().as_ref().map(|s| (s.pos, s.inputs)) else {
        return;
    };
    let view = view_at(pos);
    let inverted = view.as_ref().is_some_and(|v| v.switch.inverted);
    // `removed`: however it closed, what it says goes to the server.
    if game.native_mod_screen() != Some(MOD_ID) {
        *screen() = None;
        send(game, pos, &was_open, inverted);
        return;
    }
    if flip {
        send(game, pos, &was_open, !inverted);
    }
    let mut guard = screen();
    let Some(s) = guard.as_mut() else {
        return;
    };
    let mut due = false;
    if let Some(v) = &view {
        due = s.inputs.tick();
        s.inputs.update_boxes(v.kind, v.switch.min, v.switch.max);
    }
    let inputs = s.inputs;
    drop(guard);
    if due {
        send(game, pos, &inputs, inverted);
    }
}
