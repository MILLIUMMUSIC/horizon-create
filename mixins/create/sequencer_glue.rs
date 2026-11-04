// Horizon's side of the sequenced gearshift (rules in sequencer.rs): its block
// entity's tick, the kinetics, and its configuration screen.

use super::Game;
use super::create_kinetics::{Dir, Pos};
use super::create_sequencer::{self as rules, Instruction, Kind, MODIFIERS, Run, Sequencer};
use horizon_client::containers::ModBlockEntity;
use horizon_world::nbt::{JavaString, Tag};
use std::sync::Mutex;

pub(super) const BLOCK: &str = "create:sequenced_gearshift";
const MOD_ID: &str = "create";
pub(super) const CHANNEL: &str = "create:screens";

fn load(be: &ModBlockEntity) -> Sequencer {
    let mut s = Sequencer::default();
    let Some(Tag::String(text)) = be.nbt.get(&JavaString::from("Sequence")) else {
        return s;
    };
    let text = text.to_string_lossy();
    let mut parts = text.split('|');
    let head: Vec<&str> = parts.next().unwrap_or("").split(' ').collect();
    if let [current, duration, progress, timer, previous] = head.as_slice() {
        s.current = current.parse().unwrap_or(-1);
        s.duration = duration.parse().unwrap_or(-1);
        s.progress = progress.parse().unwrap_or(0.0);
        s.timer = timer.parse().unwrap_or(0);
        s.powered_previously = *previous == "1";
    }
    let rows: Vec<Instruction> = parts
        .next()
        .unwrap_or("")
        .split(';')
        .filter_map(|r| {
            let f: Vec<i32> = r.split(',').filter_map(|v| v.parse().ok()).collect();
            let [k, m, v] = f.as_slice() else {
                return None;
            };
            Some(Instruction { kind: Kind::from_index(*k), modifier: (*m).clamp(0, 3) as usize, value: *v })
        })
        .collect();
    if !rows.is_empty() {
        s.instructions = rows;
    }
    s
}

fn save(be: &mut ModBlockEntity, s: &Sequencer) {
    let rows: Vec<String> =
        s.instructions.iter().map(|i| format!("{},{},{}", i.kind.index(), i.modifier, i.value)).collect();
    let text = format!(
        "{} {} {} {} {}|{}",
        s.current,
        s.duration,
        s.progress,
        s.timer,
        u8::from(s.powered_previously),
        rows.join(";")
    );
    be.nbt.insert(JavaString::from("Sequence"), Tag::String(text.as_str().into()));
}

/// What a `Run` does to the block and the kinetics.
fn apply(game: &mut Game<'_>, pos: Pos, run: Run, signal: bool) {
    match run {
        Run::Stay => {}
        Run::Started { index, modifier, context } => {
            super::create_glue::set_sequencer(game, pos, modifier, context.map(|(k, v)| (k == Kind::TurnAngle, v)), true);
            game.native_set_block_property(pos, "state", &(index + 1).to_string());
        }
        Run::Ended { was_turning } => {
            super::create_glue::set_sequencer(game, pos, 0, None, was_turning);
            if !signal {
                game.native_set_block_property(pos, "state", "0");
            }
        }
    }
}

/// `SequencedGearshiftBlockEntity.tick`, `onSpeedChanged` and, with a look at the
/// signal, `SequencedGearshiftBlock.tick`.
pub(super) fn tick(game: &mut Game<'_>, pos: Pos, entity: &mut ModBlockEntity) {
    let mut s = load(entity);
    let dimension = game.dim.id().to_owned();
    let (speed, theoretical) = super::create_glue::speeds(game, &dimension, pos);
    let signal = super::create_glue::signal(game, pos);
    let running = game
        .pack
        .assets
        .registry
        .state(game.world.block(pos[0], pos[1], pos[2]))
        .is_some_and(|b| b.properties.get("state").is_some_and(|v| v != "0"));
    let last = match entity.nbt.get(&JavaString::from("LastSpeed")) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    };
    let changed = s.speed_changed(last, speed, theoretical);
    apply(game, pos, changed, signal);
    let (run, clear) = s.redstone(signal, running, speed, theoretical);
    apply(game, pos, run, signal);
    if clear {
        game.native_set_block_property(pos, "state", "0");
    }
    let ticked = s.tick(speed, theoretical);
    apply(game, pos, ticked, signal);
    entity.nbt.insert(JavaString::from("LastSpeed"), Tag::Float(speed.to_bits()));
    save(entity, &s);
    // Clients read its rows for the screen, in bytes.
    let mut view = Vec::new();
    for i in &s.instructions {
        view.push((String::new(), (i.kind.index() as u32) | (i.modifier as u32) << 4));
        view.push((String::new(), (i.value & 255) as u32));
        view.push((String::new(), (i.value >> 8 & 255) as u32));
    }
    super::create_machines::note_view(game, pos, view);
}

/// A row of a player's configuration (`ConfigureSequencedGearshiftPacket`): the
/// first starts over, stopping what runs.
pub(super) fn configure(game: &mut Game<'_>, pos: Pos, value: i32) {
    let Some((row, _, instruction)) = Instruction::decode(value) else {
        return;
    };
    let dimension = game.dim.id().to_owned();
    let (_, theoretical) = super::create_glue::speeds(game, &dimension, pos);
    let signal = super::create_glue::signal(game, pos);
    let run = game.native_with_mod_block_entity(pos, |be| {
        let mut s = load(be);
        let run = if row == 0 {
            s.configure(vec![instruction], theoretical)
        } else {
            if row == s.instructions.len() {
                s.instructions.push(instruction);
            }
            Run::Stay
        };
        save(be, &s);
        run
    });
    if let Some(run) = run {
        apply(game, pos, run, signal);
    }
}

/// `SequencedGearshiftScreen`, as this client edits it.
struct Screen {
    pos: Pos,
    rows: Vec<Instruction>,
}

static SCREEN: Mutex<Option<Screen>> = Mutex::new(None);
/// What the screen's hooks leave for the next tick, which can change the game.
static OUTBOX: Mutex<Outbox> = Mutex::new(Outbox { send: None, close: false, sound: None });

struct Outbox {
    send: Option<(Pos, Vec<Instruction>)>,
    close: bool,
    /// `scroll_value`'s pitch.
    sound: Option<f32>,
}

fn screen() -> std::sync::MutexGuard<'static, Option<Screen>> {
    SCREEN.lock().unwrap_or_else(|e| e.into_inner())
}

fn outbox() -> std::sync::MutexGuard<'static, Outbox> {
    OUTBOX.lock().unwrap_or_else(|e| e.into_inner())
}

/// `useItemOn`: the wrench passes, a kinetic block's item towards a shaft passes;
/// else the screen opens. True when it took the click.
pub(super) fn click(game: &mut Game<'_>, pos: Pos, face: [i32; 3]) -> bool {
    let held = game.held().map(|s| s.item.to_string());
    if held.as_deref() == Some(super::create_glue::WRENCH) {
        return false;
    }
    let shaft = super::create_glue::kind_of(game, pos).is_some_and(|k| k.has_shaft_towards(Dir::nearest(face)));
    let kinetic_item = held.as_deref().is_some_and(|i| {
        game.pack.assets.registry.default_state(i).is_some_and(|d| {
            game.pack.assets.registry.state(d).is_some_and(|s| {
                super::create_kinetics::Kind::from_block(&s.name, |k| s.properties.get(k).map(String::as_str)).is_some()
            })
        })
    });
    if kinetic_item && shaft {
        return false;
    }
    // The hook runs on the server's side: the player's client opens the screen.
    if let Some(player) = game.native_acting_player() {
        let mut message = b"S1".to_vec();
        for v in pos {
            message.extend_from_slice(&v.to_le_bytes());
        }
        if let Err(e) = game.native_net_send_to_player(player.uuid, CHANNEL, &message) {
            log::debug!("Create: sequencer screen: {e}");
        }
    }
    true
}

/// `create:screens` on a client: the server says to open it.
pub(super) fn from_server(data: &[u8]) {
    let Some(rest) = data.strip_prefix(b"S1") else {
        return;
    };
    if rest.len() != 12 {
        return;
    }
    let int = |i: usize| i32::from_le_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
    *OPENING.lock().unwrap_or_else(|e| e.into_inner()) = Some([int(0), int(4), int(8)]);
}

/// The screen to open at the next tick of the window's client.
static OPENING: Mutex<Option<Pos>> = Mutex::new(None);

fn open(game: &mut Game<'_>, pos: Pos) {
    let rows = super::create_machines::client_slots(pos).map_or_else(Vec::new, |slots| {
        slots
            .chunks_exact(3)
            .map(|c| Instruction {
                kind: Kind::from_index((c[0].1 & 15) as i32),
                modifier: (c[0].1 >> 4 & 3) as usize,
                value: (c[1].1 | c[2].1 << 8) as i32,
            })
            .collect()
    });
    let rows = if rows.is_empty() { Sequencer::default().instructions } else { rows };
    if game.native_mod_screen_open(MOD_ID) {
        *screen() = Some(Screen { pos, rows });
    }
}

const WIDTH: f32 = 173.0;
const HEIGHT: f32 = 161.0;

fn corner(rect: [f32; 4]) -> [f32; 2] {
    // setWindowSize, setWindowOffset(-20, 0).
    [rect[0] + (rect[2] - WIDTH) / 2.0 - 20.0, rect[1] + (rect[3] - HEIGHT) / 2.0]
}

/// The input under the mouse: row and which of its three.
fn input_at(rows: &[Instruction], at: [f32; 2], mouse: [f32; 2]) -> Option<(usize, usize)> {
    let (x, y) = (mouse[0] - at[0] - 30.0, mouse[1] - at[1] - 20.0);
    if y < 0.0 {
        return None;
    }
    let row = (y / 22.0) as usize;
    if row >= rows.len() || y - row as f32 * 22.0 > 18.0 {
        return None;
    }
    let def = rows[row].kind;
    if (0.0..50.0).contains(&x) {
        Some((row, 0))
    } else if (58.0..86.0).contains(&x) && def.value_range().is_some() {
        Some((row, 1))
    } else if (88.0..116.0).contains(&x) && def.has_speed() {
        Some((row, 2))
    } else {
        None
    }
}

fn over_confirm(at: [f32; 2], mouse: [f32; 2]) -> bool {
    let (x, y) = (mouse[0] - at[0] - (WIDTH - 33.0), mouse[1] - at[1] - (HEIGHT - 24.0));
    (0.0..18.0).contains(&x) && (0.0..18.0).contains(&y)
}

/// `native_screen_render_post`: `renderWindow`, its inputs' tooltips.
pub(super) fn render(game: &Game<'_>, batch: &mut horizon_client::hud::HudBatch, rect: [f32; 4], mouse: [f32; 2]) {
    if game.native_mod_screen() != Some(MOD_ID) {
        return;
    }
    let guard = screen();
    let Some(s) = guard.as_ref() else {
        return;
    };
    let at = corner(rect);
    let [x, y] = at;
    let sheet = "create:gui/sequencer";
    game.native_sprite(batch, sheet, [0.0, 0.0, WIDTH, HEIGHT], [x, y, WIDTH, HEIGHT]);
    for row in 0..5 {
        let offset = 22.0 * row as f32;
        let src_y = s.rows.get(row).map_or(104.0, |r| r.kind.background_y());
        game.native_sprite(batch, sheet, [0.0, src_y, 162.0, 22.0], [x, y + 16.0 + offset, 162.0, 22.0]);
        let Some(r) = s.rows.get(row) else {
            continue;
        };
        let label = |batch: &mut horizon_client::hud::HudBatch, dx: f32, text: &str| {
            game.native_text(batch, text, [x + dx, y + 26.0 + offset - 1.0], 0xFF_FF_EE, true);
        };
        label(batch, 36.0, r.kind.label());
        if r.kind.value_range().is_some() {
            let text = r.kind.format(r.value);
            let width = game.native_text_width(&text).floor();
            label(batch, 90.0 + (12.0 - (width / 2.0).floor()), &text);
        }
        if r.kind.has_speed() {
            label(batch, 127.0, MODIFIERS[r.modifier].1);
        }
    }
    let title = "Sequenced Gearshift";
    let centre = x + ((WIDTH - 8.0) / 2.0).floor() - (game.native_text_width(title) / 2.0).floor();
    game.native_text(batch, title, [centre, y + 4.0], 0x59_24_24, false);
    // IconButton: the button and I_CONFIRM on it.
    let (bx, by) = (x + WIDTH - 33.0, y + HEIGHT - 24.0);
    let hover = over_confirm(at, mouse);
    game.native_sprite(batch, "create:gui/widgets", [if hover { 18.0 } else { 0.0 }, 0.0, 18.0, 18.0], [bx, by, 18.0, 18.0]);
    game.native_sprite(batch, "create:gui/icons", [0.0, 16.0, 16.0, 16.0], [bx + 1.0, by + 1.0, 16.0, 16.0]);
    // renderAdditional: the block, five times its size.
    game.native_item(batch, BLOCK, [x + WIDTH + 6.0, y + HEIGHT - 56.0], 80.0);
    if let Some((row, input)) = input_at(&s.rows, at, mouse) {
        let r = s.rows[row];
        let mut lines: Vec<String> = Vec::new();
        match input {
            0 => {
                lines.push("Instruction".to_owned());
                lines.extend(Kind::ALL.iter().map(|k| format!("{} {}", if *k == r.kind { "->" } else { "  " }, k.description())));
            }
            1 => {
                lines.push(r.kind.parameter().to_owned());
                lines.push(r.kind.format(r.value));
            }
            _ => {
                lines.push("Speed, Direction".to_owned());
                lines.extend(MODIFIERS.iter().enumerate().map(|(i, m)| format!("{} {}", if i == r.modifier { "->" } else { "  " }, m.2)));
            }
        }
        lines.push("Scroll to Modify".to_owned());
        let width = lines.iter().map(|l| game.native_text_width(l)).fold(0.0, f32::max) + 8.0;
        let (tx, ty) = (mouse[0] + 10.0, mouse[1] - 8.0);
        game.native_fill(batch, [tx, ty, width, lines.len() as f32 * 10.0 + 6.0], 0xF0_10_00_18);
        for (i, line) in lines.iter().enumerate() {
            game.native_text(batch, line, [tx + 4.0, ty + 3.0 + i as f32 * 10.0], 0xFF_FF_FF, true);
        }
    }
}

/// `native_screen_mouse_scrolled_pre`: the input under the mouse.
pub(super) fn scrolled(game: &Game<'_>, rect: [f32; 4], mouse: [f32; 2], up: bool, shift: bool) -> bool {
    if game.native_mod_screen() != Some(MOD_ID) {
        return false;
    }
    let mut guard = screen();
    let Some(s) = guard.as_mut() else {
        return false;
    };
    let Some((row, input)) = input_at(&s.rows, corner(rect), mouse) else {
        return true;
    };
    let r = s.rows[row];
    let changed = match input {
        0 => {
            let next = rules::scroll_option(r.kind.index(), 5, up);
            if next != r.kind.index() {
                rules::change_kind(&mut s.rows, row, Kind::from_index(next));
            }
            next != r.kind.index()
        }
        1 => {
            let next = rules::scroll_value(r.kind, r.value, up, shift);
            s.rows[row].value = next;
            next != r.value
        }
        _ => {
            let next = rules::scroll_option(r.modifier as i32, 4, up) as usize;
            s.rows[row].modifier = next;
            next != r.modifier
        }
    };
    if changed {
        outbox().sound = Some(1.5);
    }
    true
}

/// `native_screen_mouse_button_pressed_pre`: the confirm button.
pub(super) fn pressed(game: &Game<'_>, rect: [f32; 4], mouse: [f32; 2], button: i32) -> bool {
    if game.native_mod_screen() != Some(MOD_ID) || screen().is_none() {
        return false;
    }
    if button == 0 && over_confirm(corner(rect), mouse) {
        outbox().close = true;
    }
    true
}

/// `removed`: the screen closes, its rows go to the server.
pub(super) fn closing() {
    if let Some(s) = screen().take() {
        outbox().send = Some((s.pos, s.rows));
    }
}

/// Once a tick on the window's client: what the screen's hooks left.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    if let Some(pos) = OPENING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        open(game, pos);
    }
    let (send, close, sound) = {
        let mut o = outbox();
        (o.send.take(), std::mem::take(&mut o.close), o.sound.take())
    };
    if close {
        game.native_mod_screen_close(MOD_ID);
        closing();
    }
    // Escape closes it without a hook we can rely on being told first.
    if game.native_mod_screen() != Some(MOD_ID) {
        closing();
    }
    if let Some(pitch) = sound
        && let Some(p) = game.player.as_ref()
    {
        let at = [p.position.x, p.position.y, p.position.z];
        game.native_play_local_sound("create:scroll_value", at, 0.124, pitch, 0);
    }
    let send = send.or_else(|| outbox().send.take());
    if let Some((pos, rows)) = send {
        for (row, i) in rows.iter().enumerate() {
            super::create_glue::send_value(game, pos, i.encode(row, rows.len()));
        }
    }
}
