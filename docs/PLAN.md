# Porting Create to Horizon: the plan

Create 6.0.10 (1.21.1, NeoForge): ~2,000 Java files, plus Flywheel (instanced rendering), Ponder (scenes) and
Registrate (registration). Horizon is Minecraft 26.3 in Rust: no JVM, so every line of behaviour is ported to Rust.

`horizon-port-pack check` on the jar, the starting inventory:

| Carried by Horizon already (data) | Left to port (code) |
|---|---|
| 542 blocks (template blocks), 142 items, 931 recipes, 640 loot tables, 73 worldgen files, 1,293 textures | 101 blocks with no vanilla template, 130 block entities, 23 menus, 26 screens, 21 renderers, 11 entities, 5 recipe types (879 recipes), 13 particle types, 3 network messages, 30 events, 64 Mixins |

## Architecture

- **Rules in plain Rust** (`kinetics.rs` and the modules after it): a port of Create's classes over a small `Level`
  trait, unit-tested with `cargo test`, no Horizon types. Block entities are rows keyed by position, one table per
  dimension, as `TorquePropagator` keeps one network map per level.
- **Glue** (`glue.rs`): Horizon's native hook points (block changes, level ticks, right clicks, world lifecycle,
  the mod message channel) plus source mixins where no hook exists yet (drawing). The bundle pins one Horizon
  Nightly; each hook point we need upstream is one less pinned mixin.
- **Content from the player's jar** (`tools/prepare_content.py`): Create's assets are All Rights Reserved, so they
  are never redistributed.
- **Fidelity**: Create's code is the spec. Where the port knowingly differs, the code says so. Next step for
  proof: a NeoForge oracle (Horizon ships one for data, `tools/neoforge_oracle.py`) and per-feature harnesses
  comparing tick-by-tick speeds and stress.

## Milestones

1. **Kinetic core** (done in this repo): propagation, networks and stress, generators (creative motor, hand
   crank), relays (shaft, cogwheels, gearbox), a consumer (press), saving, client sync, rotating parts drawn.
2. **Visual completeness of kinetics** (done): shaft halves on motors and gearboxes (gearbox halves turn by
   their source's side, as `GearboxRenderer`), the hand crank's base and handle (the handle chases the network's
   speed, as `HandCrankBlockEntity.independentAngle`), the press's shaft and head (at rest), engineer's goggles
   overlay (stress impact, generator capacity, network stress, overstressed). Left for later: encased shafts and
   cogs (no vanilla template has `top_shaft`/`bottom_shaft`), rotation particles and sounds.
3. **First processing chain**, part 1 (done): the depot (held item and 8-slot output buffer as Horizon container
   slots: hoppers, breaking and saving work as for any container; items placed from the top, falling on it, given
   back to the player), the press over a depot or over items on the ground (`PressingBehaviour`'s 240-tick cycle,
   `create:pressing` recipes, head animation), the millstone (`create:milling`, items thrown on it, outputs to an
   empty hand), the water wheel (flow score from the fluid's flow around it, 8 RPM, 32 su).
   Part 2 (checked in the game in part: see VALIDATION.md):
   - Encased fan (`fan.rs`, `AirCurrent`): 2 su, shaft behind it; its current's reach from its speed (3 to 20
     blocks), stopped by full blocks and reaching into partial ones (`findMaxDepth`), passing `create:fan_transparent`
     blocks; segments of splashing (water), haunting (soul fire, lit soul campfire), smoking (fire, lit campfire)
     and blasting (lava), by Create's priorities; item entities in it pushed along and processed after 150 ticks
     per 16 items (`create:splashing`, `create:haunting`, vanilla smoking, smelting/blasting, items without a
     smelting result burnt unless fire resistant); a depot's held item in it (or a block below a horizontal
     current) processed the same way. Rebuilt every 30 ticks and when its speed changes. The propeller partial
     turns at five times the speed (80 to 1280). Mobs in the current are pushed (from rest: Horizon gives no mob's motion), set on fire, hurt (vanilla damage types standing in for Create's), blinded and slowed, horses haunted into skeleton horses; players in it are hurt and blinded through commands, and pushed on the window's own side (`fan_client_tick`, from the current its client builds; an eighth as hard sneaking, not when flying in creative), as by a nozzle. Engine gaps: the air particles and the fan's sound (Create's own particle type and sound event: Horizon has no mod particles and plays vanilla sounds only); a guest player on a LAN server isn't pushed (only the window's player moves on its own side). A nozzle on a fan's face pushes mobs and items around it away or draws them in (`NozzleBlockEntity`), and processing hands back inputs' crafting remainders (`applyRecipeOn(.., true)`: pressing, splashing, haunting, deploying).
   - Mechanical mixer and basin: the mixer (4 su, a small cog, needs 30 RPM) runs `MechanicalMixerBlockEntity`'s
     40-tick cycle with its `processingTicks` (`log2(512 / speed) * 15 + 1` at the bottom) over a basin two below;
     the basin is a mod block entity with 9 input and 9 output slots (hoppers in and out, items thrown in, an empty
     hand takes all). `create:mixing` recipes with items only and no heat (andesite alloy from iron or zinc
     nuggets...), most ingredients first, results rolled into the output slots (all must fit), mixing again while
     another batch is there. Pole and head partials go down and up, the head whisking at the head's speed.
     Skipped and noted: fluids (dough, mud, chocolate, tea, honey, lava, cardboard pulp: fluid ingredients or
     results), heat (no blaze burner yet: brass and the other heated recipes), shapeless crafting recipes in the
     mixer (`allowShapelessInMixer`), potion brewing, the basin's filter slot, pouring out sideways (spouts: the
     template has no `facing`, so it keeps its outputs), its fluid tanks and their rendering. Basins draw their
     items as `BasinRenderer` places them (no ingredient spin yet).
   - Value boxes: the creative motor's speed (`ScrollValueBehaviour`, `MotorValueBox` on its sides): looking at
     the box shows its value over the hotbar, the mouse wheel changes it (Create 0.5's steps: 1, 4, 16, 64; never
     0), sent on the mod channel `create:values` and checked by the server (reach, game mode, range). Create 6
     opens a value board on a held right click instead (engine gap below). Right-click stepping is kept.
   - Wrench (`IWrenchable`): turns shafts, cogwheels, gearboxes (about the clicked face), motors, cranks, water
     wheels, fans (a quarter turn off the face's axis) and presses (from above or below); sneaking takes ours into
     the inventory (not in creative), a block entity's items dropped. Through `native_use_item_on`, and through
     `native_right_click_block` for blocks whose template has a use of its own (press, mixer, millstone, depot,
     basin), which would take the click first. A wrench in the main hand of a player not in creative breaks a minecart in one blow (`wrenchInstaKillsMinecarts`: the window's player through a cancellable hook on `Game::attack_cart`, guests through `native_attack_entity`). Engine gaps: its sounds (mod sound events) and its own render (`WrenchItemRenderer`'s turning gear: a mod's own item model, horizon-rust/horizon#7488).
   - Water wheel model (`obj.rs`): an OBJ/MTL loader reads `wheel.json` (its `model`, `flip_v`, texture parents),
     `water_wheel.obj` and `.mtl` from the game's resources (the content mod made from the player's jar) at
     runtime, client side, and draws it turned with `rotateToFaceVertical` and spun with the network, in spruce
     (the default material: oak planks and log swapped for spruce's). The block's own model (its frame) stays in
     the chunk mesh. Planks (vanilla woods) clicked on a wheel make it of their wood. The large water wheel (`LargeWaterWheelBlock`, 128 su, 4 RPM from the flow around it at two blocks) keeps its eight invisible structure blocks (`WaterWheelStructuralBlock`, passing clicks on); broken or blocked, it breaks. Another large wheel on its negative side draws the longer axle (`EXTENSION`, `WaterWheelRenderer`'s `LARGE_EXTENSION`), and it is placed only where the eight blocks around it across its axis can be replaced (`getStateForPlacement`; else nothing is placed and "Clear Blocks for Placement" shows in red on the action bar, `LargeWaterWheelBlockItem`). The space it needs is outlined in red (`showBounds`), through the outlines below.
4. **Logistics**. Part 1 (**compiled and unit-tested only, not yet run in the game**):
   - Mechanical belt (`belt.rs`: `BeltBlock`, `BeltBlockEntity`, `BeltInventory`, `TransportedItemStack`,
     `BeltHelper`, `ItemHandlerBeltSegment`, `BeltConnectorItem`; `logistics.rs`): the belt connector joins two
     shafts of one axis in a line (horizontal, or sloped one up per block), up to 20 blocks, through replaceable
     blocks and shafts (which become pulleys); the first click remembers the shaft (an action bar line says so:
     there is no preview line). Each segment is a kinetic block (`Kind::Belt`): pulleys connect to shafts along
     the belt's axis, segments of one belt pass their speed on (`propagateRotationTo`), sloped ones diagonally; no
     stress impact (`CStress.setNoImpact`). The controller (its start) keeps the items: they move at
     `speed / 480` blocks a tick, keep a block apart, stop at walls (`BLOCKED`), hand over to the next belt, a depot,
     a chute or a collecting funnel facing up (`INSERT`) and are thrown off the end otherwise (`EJECT`). Items
     dropped on a belt go on it (`updateEntityAfterFallOn`), an empty hand takes a segment's items back, a shaft on
     a middle segment makes a pulley. A press over a belt holds and presses its items one at a time with the
     depot's callbacks (`BeltPressingCallbacks`, shared with the depot now); a fan's current processes items on
     belt segments (`handleProcessingOnAllItems`) as on depots. Breaking a segment breaks the chain into shafts
     (at pulleys) and air and drops the items (`onRemove`, `ejectAll`); it drops the connector, and a shaft where
     it had a pulley. Drawn as `BeltRenderer` does: its partials turned its way, their texture swapped for the
     scrolling sprite and moved with the speed (`shiftUVScrolling`), the pulley turning, and the items lying
     along it, moved on between the server's updates (`create:belts` channel). Saved beside the world
     (`data/create_logistics_*.txt`). A dye clicked on a belt colours all of it, a water bucket washes it
     (`applyColor`; kept per segment as the kinetic entity's `material`, sent in K3's `aux`, drawn with the dyed
     scrolling sprites). Items lie in a stack of `log2(count) / 2` more copies within 16 blocks (block items spread
     by `new Random(angle)`), and `create:upright_on_belt` items stand facing the camera. Andesite and brass
     casing clicked on a segment case it (`setCasingType`, kept beside the dye; its blockstate model drawn over the
     belt as partials, the brass sprite swapped for andesite's as `BeltModel` does). The wrench (`BeltSlicer`,
     with the hit point the client sent): on an end's outer half it shortens the belt (the items past it thrown
     off, or moved back to the new start), elsewhere it splits it in two (a survival player pays a connector and
     a shaft per new end), and otherwise takes a casing off or turns a pulley back into a middle segment (its
     shaft back). The connector on an end's outer half (`useConnector`) adds a segment beyond it (of its colour),
     or joins the belt there (`beltStatesCompatible`; flipped to run the same way, `flipBelt`; its colour taken,
     the items of both moved to the joined belt's start, the two shafts and the connector given back). Mobs and the window's player
     standing on a belt ride it (`BeltMovementHandler.transportEntity`: `speed / 480` a tick along its axis, up and
     down slopes, centred unless the player walks; not while sneaking; the server moves mobs, the window moves its
     player as Create's client does). Vertical belts (between shafts one above the other) and sideways belts
     (between upright shafts) have blocks of their own (`hz_belt_<part>_vertical`/`_sideways`, BeltShapes' upright
     boxes) and are drawn turned as `BeltRenderer` turns them, their pulleys too. A cased belt under a block covering it (`isBlockCoveringBelt`) draws
     `BeltModel`'s cover; on a belt slower than half a block a tick, a rider stops behind another entity half a block
     ahead (`transportEntity`'s collisions: mobs on the server, the window's player on its side); a water bottle
     washes a belt as a water bucket does (`GenericItemEmptying`). The slicer shows nothing while hovering: Create
     6.0.10's `Feedback` is never filled ("TODO: Populate feedback"). Crushing wheels' controllers (not covering it)
     come with the crushers (milestone 7). Engine gaps: the cased shapes (shapes only by a template property; the
     casing lives in the kinetics), LAN guests riding belts (no hook moves a guest player), sounds (mod sound events).
   - Chute (`chute.rs`: `ChuteBlockEntity`'s item and `handleDownwardOutput`/`handleInputFromAbove`; a mod block
     entity): items fall at 4 blocks a second, into the inventory below (containers, depots, belt segments), into
     the chute below, or out of the bottom onto the ground; an empty chute takes up to 16 from the inventory above
     and items falling on top; its item drawn at its height in a window (`ChuteRenderer`). A fan facing up under a
     column pushes, one facing down on top pulls (`calculatePush`/`calculatePull`, through diagonal chutes too,
     `getItemMotion`): items blown up go into the inventory or chute above or fly out of the top
     (`handleUpwardOutput`), and an upward chute takes from the inventory below (`handleInputFromBelow`). An empty upward
     chute takes the middle item of a belt within reach below or draws in an item entity under it (`updateAirFlow`,
     `extractFromBelt`, `findEntities`; reach from `pull - push`). Diagonal chutes: placed on a block's side (not
     sneaking) a chute faces it and leads down to the chute beside below, else turns straight; they hand items on
     (`getTargetChute`, `getInputChutes`) and drop none. Shapes from the chutes around (`updateChuteState`, a tick
     after a change), a block per shape on a lightning rod's template (`facing`) keeping the chute's item; the wrench
     makes a window (`onWrenched`), industrial iron encases it. Smart chute (`SmartChuteBlock`, `SmartChuteBlockEntity`):
     straight, its filter on its four sides (clicked with the item, an empty hand clears it; drawn there) and its
     amount (the wheel, up to 64 at a time) kept in its block entity's data; it takes and hands on only what passes,
     stops while powered by redstone around it (read each tick: a template's `powered` would give off a signal), and
     draws its item only at its bottom. A chute under one leading off sideways breaks
     (`canSurvive`); the goggles show the chute's fans and which way items move and what it holds
     (`addToGoggleTooltip`, the fans' push and pull sent with its item); the smart chute is drawn by its `POWERED`
     model (blank blockstate, `hz_part_smart_chute[_powered]`). Engine gaps: hoppers inserting into a smart chute past
     its filter (Horizon's container path), `ExtractionCountMode.EXACTLY` (set only on the value settings board: a
     mod screen with a free cursor).
   - Andesite funnel (`funnel.rs`; no block entity): extracting (placed normally) puts one item every 8 ticks
     from the inventory behind in front of it, unless an item still lies there; collecting (placed sneaking, or
     wrenched) puts items at its opening into the inventory behind; a horizontal one over a belt is a belt
     funnel (`getShapeForPosition`): pulling items off the belt into the inventory behind
     (`BeltFunnelInteractionHandler`), or pushing one at a time onto it, by its shape and the belt's movement. The
     wrench switches it. An item clicked into a collecting andesite funnel's opening goes into the inventory behind. A funnel above a belt or depot has Create's belt funnel shape as its own `shape` state property (`retracted` along the belt, `pushing` when it extracts and `pulling` when it doesn't across it, `wall` elsewhere: `BeltFunnelBlock.getShapeForPosition`/`updateShape`, the shapes from `AllShapes`, the models from Create's belt funnel blockstate), with the belt funnel's flap (`FunnelBlockEntity.getFlapOffset`). The extended shape works too (the wrench swaps retracted and extended, a sloped belt keeps it retracted, an extended funnel stays so along the belt, tunnels' windows look for it: `extended_funnel`). Not yet: upright items beside an extended funnel (`getShapeForPosition`'s item shapes). Brass funnels: part 3. Funnels and tunnels have block entities (any template, #5549): their flaps swing as
     items pass (`FunnelBlockEntity.flap`, `BeltTunnelBlockEntity.flap`: told to clients in the block entity's
     update tag) and are drawn by `native_block_entity_submit` as `FlapStuffs.renderFlaps` does, chasing back to rest.
   Fluids (run in the game: see VALIDATION.md):
   - Fluid pipes (`fluids.rs`: `FluidTransportBehaviour`, `PipeConnection` with its three layers, `FlowSource`,
     `FluidNetwork`, `FluidPropagator`, `OpenEndedPipe`; `fluid_glue.rs`): pipes open on the six sides
     `FluidPipeBlock.updateBlockState` gives them (their own state properties, a chain's template): placed towards
     what they connect to (pipes, tanks, basins, pumps along their axis, cauldrons, hives) or along the placer's look
     (`getStateForPlacement`), turned as neighbours change (`updateShape`, `native_update_shape`). Each side also holds
     what it draws (`getRenderedRimAttachment`: rim, drain, connection, detailed connection), so Create's blockstate,
     its rims, drains and connections and the casing over a junction (`PipeAttachmentModel`) are drawn in the chunk. Flows start where there is pressure, take
     `1/32 + pressure/128` of a block a tick to fill a pipe, flip when the pressure does, and two fluids meeting break
     the pipe (water and lava leave cobblestone). A network from each endpoint feeding a pipe walks the flows to the
     endpoints they reach and moves `pressure / 2` mB a tick, split between them as Create does
     (`accumulatedFill`). Open ends draw a source block a bucket at a time (keeping the rest in their 1000 mB
     buffer), drain full water and lava cauldrons and full beehives (their honey is only a name here: tanks and basins
     hold it, nothing pours it), and pour: a source block once a bucket has gathered, waterlogging what takes water, obsidian, cobblestone or
     stone against the other fluid, nothing in the nether for water. Pipes are saved beside the world; their flows
     and pressure come back from the pumps.
   - Mechanical pump (`PumpBlockEntity`): a small cog about its facing with no shaft, 4 su; pushes out of its front
     and pulls at its back whatever its speed's sign (Create 6 decides by `FACING` alone; the wrench turns it
     round). `distributePressureTo` and `searchForEndpointRecursively` as in Create: its speed as pressure on every
     face on a way to an endpoint within 16 blocks (`mechanicalPumpRange`), split between parallel branches;
     redistributed when its speed changes, a pipe near it changes, or a block in front of a straight pipe's open
     end does. Its cog partial turns with it.
   - Fluid tank (`FluidTankBlockEntity`, `ConnectivityHandler` in `tank.rs`): tanks join into multiblocks up to 3
     wide and 32 high (`formMulti`, largest first; different fluids and the creative tank stay apart), split when a
     block goes (`splitMulti`: the controller keeps a block's worth, the rest handed out bottom up, all of it when the
     controller went), and look again for a multiblock the next tick. Their parts and fluid live in the native mod and
     are saved beside the world; each block is drawn with `FluidTankGenerator`'s model for its place and window
     (blank blockstate, hidden `hz_tank_*` holders) with `FluidTankModel`'s connected textures (`ct.rs`) and the
     sides toward joined blocks culled, the fluid as one box through the windows (`FluidTankRenderer`,
     none with windows shut), tiled a block at a time. The wrench toggles the whole tank's windows. Buckets only in
     creative, as `FluidTankBlock.useItemOn`. The creative fluid tank (`CreativeSmartFluidTank`): a bucket sets its
     fluid, pipes drain it endlessly and fills are voided; in survival an empty bucket takes it out and clears it.
     Not yet: the boiler (milestone 7, with the steam engine), tanks on moving structures (`FluidTankMovementBehavior`),
     light from glowing fluids
     (`luminosity`: engine gap, a template's light is fixed), the fluid level's easing (`LerpedFloat`), sounds and particles on buckets.
   - Basin fluids: two input and two output tanks of 1000 mB (`SmartFluidTankBehaviour`, inputs keeping their
     fluids apart, outputs taking nothing from outside), filled by pipes and buckets; `create:mixing` recipes with
     water and lava as ingredients or results run (dough, cardboard pulp; the heated ones still wait for the blaze
     burner), each fluid ingredient from one tank as `BasinRecipe.apply` does; the fluids drawn side by side as
     `BasinRenderer` does.
   - Not yet: fluids without a block in Horizon (Create's honey, chocolate, tea, builder's tea, milk, potions) and
     their pipe effects (`OpenPipeEffectHandler`), brackets, particles and sounds, the
     hose pulley, portable fluid interfaces. The goggles show a tank's and a basin's fluids and amounts
     (`containedFluidTooltip`).
   Part 2, arm and deployer (**compiled and unit-tested only, not yet run in the game**; VALIDATION.md lists the
   checks):
   - Item vault (`vault.rs`: `ItemVaultBlock`, `ItemVaultBlockEntity`, `ConnectivityHandler`; `arms.rs`): a mod block
     entity of 20 slots per block on stone's template (no use of its own), with its own `axis` and `large` properties.
     Vaults of one axis join as Create's do (up to 3 wide, 3 times as long; `formMulti` with its search order and
     creation queue), split when a block goes and join again on their next tick. Funnels, chutes and arms reach the
     whole vault in `SameSizeCombinedInvWrapper`'s slot order; hoppers reach one block's 20 slots. Placed against a
     vault's face it takes that vault's axis unless sneaking, else the player's facing's (`getStateForPlacement`);
     the wrench from above or below takes it out of its vault and turns it. Its blockstate is blank: the native mod
     draws each block from hidden holders (`hz_vault_x`, `hz_vault_z`) with `ItemVaultCTBehaviour`'s connected
     textures (`ct.rs`, `CTModel`'s sheets: medium, or large past 2 wide, the `LARGE` property), its part sent over
     `create:arms`. On moving structures it is drawn without them. Comparators read the whole vault (`calcRedstoneFromBlockEntity`).
   - Mechanical arm (`arm.rs`: `ArmBlockEntity`, `ArmInteractionPoint` and the types for depots, belts, basins,
     chutes, deployers, millstones and funnels, `ArmAngleTarget`; `arms.rs`): a mod block entity holding its item on
     an enchanting table's template, a kinetic block (`Kind::MechanicalArm`: a small cog, 2 su). Its phases, the
     lazy tick, `getDistributableAmount` against every output, round robin (the default selection mode), belt items
     waiting while it reaches for them, an empty hand taking its item. The arm item selects points as
     `ArmInteractionPointHandler` does (on the server: the action bar says the mode), and the placed arm takes the
     points within 5 blocks. Its base, lower and upper arm, claw and grips are drawn from Create's partials with
     `ArmRenderer`'s transforms and the angles the server works out as Create's client does (`create:arms`); its held
     item in the claw. As in Create 6, chests and vaults are no arm points. The mouse wheel over an arm's side steps its selection mode (round robin, forced round robin, prefer first; saved, said on the action bar); campfires (food), composters (compostables in, bone meal out) and respawn anchors (glowstone) are points. Arms hang from ceilings (`CEILING`, their own state property: placed against a block's underside, drawn and aimed upside down, AllShapes' ceiling shape). Blaze burners (`BlazeBurnerPoint`: fuel one at a time, deposit only) and saws facing up while turning (`SawType`'s depot point) are points. Not yet: jukeboxes (Horizon keeps starting a disc's song to itself), the dance (it starts only from jukebox points: with the jukeboxes), sounds (mod sound events, an engine gap). Goggles used on an arm go on it once, the item kept (`ArmBlock.useItemOn`, saved as `Goggles`), and its claw is drawn with `ARM_CLAW_BASE_GOGGLES`. The selection is outlined in each point's mode's colour while the arm item is held, and the points of an arm looked at with the wrench (`drawOutlines`, `checkForWrench`, on the window's side: the window finds its own selection by its player's name); a left click with the arm item takes a selected point out instead of hitting the block (`leftClickingBlocksDeselectsThem`).
   - Deployer (`deployer.rs`: `DeployerBlockEntity`'s cycle, `DeployerItemHandler`, `BeltDeployerCallbacks`,
     `create:deploying` and `create:item_application`; `arms.rs`): a kinetic block (`Kind::Deployer`, 4 su, its
     shaft by `axis_along_first`) with a block per `axis_along_first` on a lightning rod's template, placed as
     `DirectionalAxisKineticBlock` places it; no block entity (no template has six facings and one), so its state
     and items are kept by the native mod and saved beside the world (`data/create_deployers_*.txt`). Its cycle and
     timings, the overflow, right click on its front swapping hands, the wrench on its front switching punch and use.
     `DeployerFakePlayer` is replaced by what Horizon's game code allows: using places a held block item through
     vanilla's placement rules (`PlaceContext`), punching breaks the block in front over cycles with the held tool's
     speed and keeps its drops; on a belt or depot below, deploying recipes apply one item per cycle as
     `BeltDeployerCallbacks` does (results not locked; the held item used, damaged or kept). Arms, funnels and chutes
     reach its hand and overflow. Pole, hand and held item are drawn as `DeployerRenderer` does, its timer running on
     the client between updates. Punching, a mob in the block in front takes the fist's 1 plus the held item's
     attack damage (`player.attack`, through Horizon's mob hooks; no armour, knockback, enchantments or tool wear,
     its drops falling where it dies). Not yet: other item uses and blocks' own uses, using items on entities
     (feeding, shearing), fist bumps, sandpaper polishing and sequenced assembly, the
     crafting remainder of a damaged tool, hoppers into it (no block entity), sounds and particles. The goggles show its mode and held item with its count above the kinetic lines (`DeployerBlockEntity.addToGoggleTooltip`).
   Part 3, brass funnel and belt tunnels (**a brass tunnel on a belt with two depots beside it was run in the game: its cross shape, flaps and the distribution to both depots; the brass funnel and the rest are compiled and unit-tested only**; VALIDATION.md
   lists the in-game checks):
   - Brass funnel (`funnel.rs`, `logistics.rs`): the andesite funnel's blocks and modes (extracting and collecting
     blocks on a lightning rod's template, the belt funnel it becomes on a belt) with its filter
     (`FilteringBehaviour`, one item): a right click on its filter side (the front of a horizontal funnel, any
     side of a vertical one: Horizon's click has no hit position for the value box) sets the held item, an empty
     hand clears it; items aren't used up. The filter decides what it extracts, collects, takes off a belt (others
     pass, or wait at a funnel facing them, as `BeltFunnelInteractionHandler`), takes from a belt's end and what an
     arm deposits into it. It extracts its filter's amount, a full stack by default whether or not an item is set
     (`getAmountToExtract`), gathered from every slot holding that item (`ItemHelper.extract`, `UPTO`); funnels and
     chutes now gather the same way. The filter's item is drawn in each filter slot as `FilteringRenderer` does
     (`FunnelFilterSlotPositioning` for the funnel block), synced on `create:depots`. Filters are saved with the
     funnels in `create_logistics_*.txt`. The mouse wheel over its filter slot steps the amount it extracts (1 to 64, `FilteringBehaviour.count`, `upTo`), saved with it. Not yet: exact amounts, filter items
     (lists, attributes, packages), matching components, the belt funnel's slot positions, the filter's tooltip.
   - Andesite tunnel (`tunnel.rs`, `belt.rs`, `logistics.rs`): a block per `shape` on a chain's template (its
     `axis` holds the horizontal axis). It turns to its belt's axis and the shape its neighbours give it
     (`getTunnelState`: T and cross where a belt across it or a depot, chute or collecting funnel takes items at
     its sides; a window between tunnels, which the wrench closes and opens); it breaks off when the belt below
     goes. Its sides and flaps (`updateTunnelConnections`) are worked out each tick. A stack entering a junction
     along the belt puts one item onto each side with a flap that takes it, and waits while one doesn't
     (`flapTunnelsAndCheckIfStuck`). Flaps are drawn at rest (`BeltTunnelRenderer`/`FlapStuffs`: the content's blockstates add four flaps on the sides
     `updateTunnelConnections` gives each shape). The flaps swing as items cross into and out of the tunnel on the belt (`flapTunnelsAndCheckIfStuck`) and as a
     junction hands one sideways. Brass tunnels flap when they distribute and the belt under a new tunnel takes
     its casing (`BeltTunnelItem.updateCustomBlockEntityTag`); a tunnel is placed only on a flat belt
     (`BeltTunnelItem.canPlace`, refused through `native_state_for_placement`). Not yet: display links (its item handler, `cap`, reaches the belt below it: funnels, chutes and arms insert and extract through it).
   - Brass tunnel (`tunnel.rs`, `logistics.rs`): the andesite tunnel's blocks, holding the stack that enters it when it
     distributes (`hasDistributionBehaviour`: a flap to the side or a connected brass tunnel) and keeping the
     next item waiting until it is empty (`stuckAtTunnel`). Connected tunnels side by side over parallel belts share
     their outputs (`gatherValidOutputs`, its own sides first); `distribute` in **Split** (the default) and
     **Round robin**, after `brassTunnelTimer` (10 ticks) or 2 for a new item; outputs are belts, depots, chutes and
     collecting funnels at the sides, or the belt's end (thrown off). An adjacent brass funnel's filter decides
     what a side takes and goes first (`testFlapFilter`, `flapFilterEmpty`). The wrench steps through the seven selection modes (standing in for the value box, as Horizon can't open the value settings board; an action bar line
     says the mode) for the whole connected group, and a tunnel placed beside others gives them its mode
     (`updateTunnelConnections`); an empty hand takes the group's stacks (`grabAllStacksOfGroup`); a broken one
     drops its stack. Mode and stack are saved in `create_logistics_*.txt`. Not yet: sided filters on the tunnel itself
     (`SidedFilteringBehaviour`); the brass tunnel's connected textures (`BrassTunnelCTBehaviour`); its window
     toggle is the wrench's (`onWrenched`), and the mouse wheel over a brass tunnel steps its selection mode in place of the value box; the goggles list the stacks the connected brass tunnels hold (`addToGoggleTooltip`, from their block entities' data); the distribution's stack drawn (`BrassTunnelRenderer`).
   Later: the remaining belt features above.
5. **Contraptions**: moving block structures (pistons, bearings, gantries, minecart contraptions). In Create a
   contraption is an entity (`AbstractContraptionEntity`) holding the moved blocks (`Contraption`), drawn by its
   renderer and colliding through `ContraptionCollider`. Horizon (as of `aa27eca`) has no mod entities and no
   collision hook, so the port keeps contraptions in the native mod instead (researched 2026-11-01):
   - **What Horizon gives**: `transient_block_geometry` draws any block state at any transform (the kinetic
     parts already use it), `native_set_block` takes blocks out and puts them back, `native_teleport_mob` /
     `native_push_mob` move mobs, the local and LAN players' positions are reachable from `crate::game`, and the
     redstone hooks (`native_block_signal`, `native_neighbor_changed`) can power them.
   - **The model** (`contraption.rs`, no Horizon types, unit-tested): `Contraption` with its blocks (local
     position, state, block entity data), its anchor and bounds; `searchMovedStructure` with the subset of
     `BlockMovementChecks` the ported blocks need (movement necessary/allowed, brittle, not supportive, attached
     towards); assembly (blocks taken out, `customBlockRemoval`), disassembly at the rounded offset
     (`customBlockPlacement`, blocks put back, drops for those that can't be), saving beside the world and a
     channel (`create:contraptions`) carrying blocks once and offsets each tick.
   - **Collision** is the open problem. Plan: entities standing on a contraption move with it (its delta each
     tick, as `ContraptionCollider` carries riders), entities in its way are pushed out along the motion; the
     player can't stand on or bump into a moving structure without engine help, so between the two the port
     puts hidden collision blocks (a barrier-like template, nothing drawn) at the cells a slow linear contraption
     covers, moved with it. This is a stand-in; the real fix is an engine hook for extra collision boxes per
     level (to propose upstream with the capability hook).
   Parts:
   1. Mechanical piston and sticky piston, extension poles, piston head (**done, checked in the game in part**):
      `contraption.rs` has `searchMovedStructure`/`moveBlock` with `BlockMovementChecks`' fallbacks (the block's
      Java classes and tags from Horizon's facts: brittle, attached towards, not supportive, slime and honey
      sticking, push reactions), `PistonContraption` (poles behind and in front, the head, the line in front,
      a sticky one pulling the first block), colliders and the world check, and `LinearActuatorBlockEntity`'s
      tick in `MOVE_PLACE` (assembly when the speed changes, `convertToLinear` clamped to ±0.49, stopping on the
      grid against the world, placed at either end or when it stops). `contraptions.rs` takes the blocks out
      (block entities' data kept, `UPDATE_MOVE_BY_PISTON`), puts them back (dropping what can't go back), keeps
      the piston's `STATE` blocks, draws the moving blocks on clients from `create:contraptions` (interpolated
      between ticks), carries what stands on them (the window's player from `native_tick` on its own side, mobs
      and items on the server), and saves running pistons beside the world. Blocks: a lightning rod's template
      per piston, `axis_along_first` and state, placed as `DirectionalAxisKineticBlock` places them; poles and
      heads on lightning rods. The movement mode (`MovementMode`, `LinearActuatorBlockEntity.tryDisassemble`: place when stopped, only in the starting position, or never until the speed changes) is stepped by the mouse wheel over the piston or the rope pulley in place of the value box, saved with it (gantry carriages have none in Create either: they aren't linear actuators; the elevator pulley's is not ported). Not yet:
      redstone and sequenced gearshifts, pistons inside other contraptions (`moveMechanicalPiston`), breaking a
      pole or the head breaking the rest, natively drawn parts other than shafts' and cogs' models, the drill's head and the harvester's blade drawn while moving (shafts and cogs don't turn), block entity renderers while moving (chests), colliding with other contraptions,
      pushing entities that stand in the way (only riders and entities inside move), LAN guests carried (the
      server can't move a remote player), sounds and the assembly error on goggles.
   2. Super glue and linear and radial chassis (what a structure takes in), the mechanical bearing and windmill
      bearing (rotating contraptions: drawn turned; collision of turned structures stays a gap).
      Part 2a, chassis and super glue (**done, checked in the game in part**; the wrench pointing at a chassis outlines what it takes in, Ctrl its group, as `ChassisRangeDisplay` does, in white lines without Create's checkered face texture: `Outliner.showCluster` is `outline.rs`' `cluster_boxes`): `contraption.rs` has
      `moveChassis` (`addAttachedChasses`, `getIncludedBlockPositions` linear and radial), the chassis' glueable
      sides (`getGlueableSide`, slime on a side and on a sticky side, `glueAllowedOnSide`, `updateShape`),
      `SuperGlueEntity.isGlued` in `moveBlock` and `searchGlueGroup` (`isValidFace`, `isSideSticky`).
      `contraptions.rs` keeps the chassis' ranges (`ChassisBlockEntity.range`, default 8, scrolled with the wrench
      held, Ctrl for the group) and the glue (two corners per box) per dimension, both saved beside the world and
      moved with the structure; super glue's corners are chosen by two right clicks on the server
      (`SuperGlueSelectionPacket`'s checks, one use of the glue), a left click takes the glue around a block off,
      and players holding glue see its boxes as thin slime edges. Blocks: a log's template keeps `axis`, each set
      of sticky sides is a hidden `create:hz_<chassis>_<sides>` dropping the plain one; linear chassis take the
      axis of a chassis beside them or the player's look. Super glue in hand outlines the glue nearby thinly and, after the first corner, the box to the hovered block and the blocks it would glue (`SuperGlueSelectionHandler.tick`'s outlines, in its colours; read from the single-player server's state, so not on a LAN guest). Super glue in the off hand glues a block placed with the main hand to the one it is placed against (`SuperGlueHandler.glueInOffHandAppliesOnBlockPlace`; where Create cancels a placement whose look meets another block, this one doesn't). Not yet: the handler's status messages and the highlighted glue under the crosshair, glue particles, the sticker, chassis' connected textures, Create 6's value board for the range (the wheel steps it here).
      Part 2b, mechanical and windmill bearings (**done, checked in the game in part**): `contraption.rs` has
      `BearingContraption` (the shared `searchMovedStructure` from the block in front, the bearing anchoring, the
      sail count and `notEnoughSails`), `MechanicalBearingBlockEntity`'s motion in `ROTATE_PLACE` (assembly on a
      speed change or an empty-hand click, `convertToAngular`, the angle snapped on a stop, a windmill put back as
      built), `StructureTransform` for putting a structure back on a quarter turn (positions, and states through
      vanilla's rotation about Y, `FACING`/`AXIS` about X and Z, stairs, face-attached blocks, half turns of
      slabs and horizontal blocks) and the windmill's generated speed. `contraptions.rs` takes the structure out,
      draws it turned about the anchor's centre on clients (the angle running on between updates), carries what
      stands on a structure turning about Y (players on their own side, as pistons do), saves turning bearings, and
      sets a windmill's speed in the kinetics (`flow_score` holds it). The bearings use a lightning rod's
      template; their tops are partials turned with the structure. The mechanical bearing's rotation mode (`RotationMode`: place when stopped, only near the initial angle, only when the anchor is destroyed; the mouse wheel over it, saved with it) is ported. Not yet: the windmill's rotation direction box (it turns
      clockwise), sequenced gearshift angles, the clockwork bearing, structures on bearings inside other
      contraptions (`StabilizedContraption`), riders on horizontal bearings, colliding with turning blocks, kinetic
      and block-entity-rendered blocks drawn while turning, rails' and stairs' shapes and bells turned, the
      assembly error on goggles.
   3. Gantry carriage and shaft, rope and elevator pulley.
      Rope pulley (**done, checked in the game in part**): `contraption.rs` has `PulleyContraption` (the search
      from the block below the rope, the rope and pulley anchoring) and `PulleyBlockEntity`'s motion as a
      `LinearActuatorBlockEntity` in `MOVE_PLACE` (`Pulley`: the rope counted, a bare rope running down until it
      reaches a block and then taking it, `visitNewPosition`; stopping on the grid against the world; the
      magnet just above the anchor not in the way going up), with the colliders shared with the piston.
      `contraptions.rs` takes the rope in and the load out, lays rope and magnet blocks back where it stops,
      draws the load moving and carries what stands on it, saves running pulleys; `glue.rs` draws the shaft,
      the coil and the moving rope and magnet (`AbstractPulleyRenderer`). Rope and magnet stand on a lightning
      rod's template for their shapes; the pulley is placed as `HorizontalAxisKineticBlock` places it. Not yet:
      mirrored pulleys sharing one load (`startMirroringOther`), the coil's scrolling texture, the movement mode
      box, the threshold switch reading it, breaking the rope.
      Elevator pulley (**done, checked in the game in part**): `contraption.rs` has `ElevatorContraption`'s
      contact (`elevator_column`: the one sideways redstone contact in the cabin gives the column and its height),
      `ElevatorColumn` (contacts in order, the floor called) and `ElevatorPulleyBlockEntity`'s motion
      (`getTargetOffset`, `getMovementSpeed`: speeding up and slowing down toward the floor). `contraptions.rs`
      assembles it when its shaft starts turning (the cabin is what stands below the gap under the pulley, the
      column's redstone contacts become elevator contacts, `gatherAll`), calls a floor when a contact gets a
      signal (`neighborChanged`), takes the cabin out and moves it as a pulley's load (drawn, riders carried),
      eases it onto the floor, and the floor's contact gives a signal (`triggerContact`). Contacts (redstone and
      elevator) are on a lightning rod's template: its `powered` is the contact's signal out. Unlike Create, the
      cabin is put back as blocks once it has arrived (contraptions aren't entities here, so nothing could stand
      on it while it waits) and taken out again for the next call; a client puts a player standing on a load
      back on top of it when it is put back. Not yet: floor names and the contact's screen, contraption controls
      choosing floors, door controls, the pulley's belt and coil drawn, columns
      saved (gathered again on load), redstone contacts on moving structures (`ContactMovementBehaviour`), mirrored elevator pulleys, actors on the cabin.
      Gantry shaft and carriage (**done, checked in the game in part**): `contraption.rs` has the shaft's parts
      (`updateShape`), `canAssembleOn`, `getPinionMovementSpeed`, `GantryContraption` (from the carriage, the
      shaft behind anchoring) and `GantryContraptionEntity`'s tick (`checkPinionShaft`: no shaft or no speed
      puts it back, passing the centre of a shaft it can't go on from puts it back; the world in the way puts it
      back; still for two ticks). `contraptions.rs` queues carriages when their shaft's speed changes, when
      placed or clicked with an empty hand, moves the structure along the shaft, draws it and carries what
      stands on it, saves moving gantries. Blocks: each shaft part on a lightning rod's template, drawn turning
      from hidden holders; the carriage as the deployer (`AXIS_ALONG_FIRST_COORDINATE` cycled to fit the shaft);
      its wheels a partial. Not yet: powered shafts (redstone, and the carriage driving the shaft through its
      pinion: the template's `powered` is the lightning's), sequenced gearshift distances, gantries on
      contraptions, the wheels' exact turn, the carriage breaking when its shaft goes.
   4. Actors on contraptions (drill, saw, harvester, plough, deployer, portable storage interfaces).
      Part 4a (**done, checked in the game in part**): `actors.rs` has `tickActors`' trigger
      (`shouldActorTrigger`, the bearing centre's 20-tick timer), `BlockBreakingMovementBehaviour` (breaking with
      progress, the structure stalled until the block breaks, falling blocks broken from the top and settling for 10
      ticks, `getBlockBreakingSpeed`), `DrillMovementBehaviour`, `HarvesterMovementBehaviour` (crops, berries, cane,
      leaves with shears, one seed kept back, replanting), `PloughMovementBehaviour` (its `canBreak`, the hoe's
      tilling), and the standing drill's `BlockBreakingKineticBlockEntity` (`Kind::Drill`, 4 su). The pistons,
      bearings, rope pulleys and gantries take a `stalled` flag. `actor_glue.rs` ticks each moving structure's
      actors before it moves, breaks blocks with their drops, harvests and tills, and ticks standing drills; the
      drill's head is a partial drawn turning in the world and still on moving structures. The harvester and plough
      stand on a grindstone's template (`facing`, `face` kept at wall). Moving drills and saws hurt what they move into (`damageEntities`, through `/damage`, the old gap is gone; Horizon's `/damage` knows vanilla's damage types only, so Create's `crush`, `mechanical_drill` and `mechanical_saw` are `minecraft:generic`, with its death message: **blocked on the engine**, no issue) and with ploughs throw it (mobs and the window's player; not LAN guests); standing drills and saws hurt what is in their cell. Not yet: the cobblestone generator
      optimisation, actors on gantries and
      pulleys checked in the game, placing a
      harvester on a wall while looking steeply up or down (it faces the player).
      Part 4b, the mechanical saw (**done, checked in the game in part**): `saw.rs` has `TreeCutter.findTree`
      (logs and roots, `validateCut`, leaves by distance, attachments, vertical plants from the top, chorus) and
      `isSawable`; `actors.rs` the saw as an actor (`SawMovementBehaviour`: it cuts only facing sideways in the
      world, fells no tree from leaves, doesn't wait for falling blocks). `Kind::Saw` (4 su) is a
      `DirectionalAxisKineticBlock` as the deployer, a block per `axis_along_first` on a lightning rod's template;
      a standing saw facing sideways breaks sawable blocks in front and fells the tree, its drops thrown away from
      it (`dropItemFromCutTree`). The blade is a partial (active, reversed or still), drawn on moving structures
      too. A saw facing up processes items (`SawBlockEntity`: a block entity with `ProcessingInventory`'s
      state on its lightning rod template; an item lands on it or comes from a belt or depot end, one at a time as
      `bulkCutting` is off, `create:cutting` recipes in round robin, the time scaled by speed, its pieces handed to
      the belt or depot ahead or thrown, the stone or wood sound by a name heuristic). Its filter (`SawFilterSlot`, a click on its top with an item, `outputMatchesFilter` over the recipes, drawn on the top) is ported. Not yet: cutting
      by vanilla stonecutting recipes (Horizon's recipe tables don't hand them to mods: an engine gap, no issue
      yet), package contents, belt funnels it hands to, the item and particles drawn on it, `flipped`, the tree's blocks' break effects by chance (all show), dynamic trees, wrenching a saw.
      Part 4c, storage, deployers and portable storage interfaces (**done, checked in the game in part**): the
      structure's storage is its chests', barrels' and shulker boxes' slots (`create:chest_mounted_storage`,
      `create:simple_mounted_storage`) in its blocks' order, read and written in their captured data
      (`MountedStorageManager.getAllItems`); actors put what they collect there first (`collectOrDropItem`, the
      saw's felled tree too). A deployer's state (held item, mode, inventory) goes with its block; on the move it
      takes an item from the storage into an empty hand, uses it or punches 2 blocks ahead
      (`DeployerMovementBehaviour`, the stationary deployer's actions), stalls while it breaks a block (striking
      every 21 ticks) and puts its other items into the storage. The portable storage interface (`Kind`-less, a
      lightning rod's template, its middle and top merged into its model) finds a stationary one facing it one or
      two blocks ahead, stalls the structure in front of it and hands the structure's storage to whatever reaches
      the stationary one through the item handler (funnels, chutes, arms) until 60 ticks pass without a transfer
      (`PortableStorageInterfaceBlockEntity`'s timer). `actors.rs` has the slot rules (`insertItem`,
      `ItemHelper.extract`), the interface's timer and the actors' ticks. Item vaults count as storage too (each
      captured block's own slots). Not yet: toolboxes, depots and creative crates as storage, double chests as one, the fluid interface, the interfaces' extension drawn moving, vanilla hoppers reaching a stationary interface (they don't go through
      the item handler), the deployer's pole and hand drawn on a moving structure, schematics. The deployer's filter works (`DeployerItemHandler.isItemValid`, a held item it doesn't take goes to the overflow, a taken held item isn't extracted; set by a click on a side face with the item, saved with its state) and is drawn on its two filter sides (`DeployerFilterSlot`'s item, without the value box's frame).
   5. Minecart contraptions (cart assembler): **blocked on the engine**, not done: they need mod entities riding
      vanilla carts and a moving group of blocks bound to an entity; upstream: horizon-rust/horizon#5556 (fake
      players, mod entity types, moving groups of blocks, open) and #5212 / #6300 (mod entity types from code, open).
6. **Trains**: track graph, signals, stations, schedules. **Blocked on the engine** (checked 2026-11-04): a carriage
   is a contraption entity that runs on tracks across chunks with its own collision and bogeys that follow curved
   track blocks, and trains need mod entity types (#5556, #5212, #6300, all open), curved track shapes keyed by
   more than a template's properties (#5550, open); the station and schedule screens could use the pin's
   `native_screen*` hooks (found 2026-11-04, see below). Not started; the
   track graph and signal rules could be written as pure modules but nothing of them could be seen or used without
   the carriage entity, so they wait for it.
7. **Everything else**: Ponder scenes (an offscreen scene renderer and a free-cursor screen: the pin has
   `native_screen*` hooks, but no scene renderer; treated as engine-blocked until one exists, no issue), schematics
   and the schematicannon (screens through `native_screen*`, to be looked at), decoration
   sets, Create's compatibility with other mods (**blocked on the engine**: no shared capabilities, #1040 for item
   storage only, nothing for fluids or energy). The rest of this milestone is ported item by item below; what is
   done is in the README's estimate.
   Done from this milestone: the redstone link (`facing`, `powered` and `receiver` are its own state properties; a
   transmitter sends the strongest signal beside it, a receiver takes the strongest transmitter of its two-item
   frequency within 256 blocks and powers the block it is mounted on through `native_block_signal` and
   `native_is_signal_source`; an item clicked on its upper or lower slot sets that frequency, sneaking with an empty
   hand or the wrench flips the mode; the two frequency items are drawn on its plate, `RedstoneLinkFrequencySlot`).
   It needs a block behind it that isn't replaceable (`canSurvive`, through
   `native_can_survive`) and breaks with its drops when that block goes (`neighborChanged`, through the shape update).
   Not yet: dyed items as different frequencies, redstone controllers and factory panels, the
   network's save across reloads (the transmitters re-announce at their first tick), `blockUpdated` on the attached
   block, and the link's range display.
   The blaze burner (`blaze.rs`: `HeatLevel`, `BlazeBurnerBlockEntity`'s fuel, burn time and
   heat, `HeatCondition.testBlazeBurner`; `heat_level` is its own state property on a stone template, its block entity
   holds the fuel; fuel in the hand (anything a furnace burns, the blaze cake, the creative blaze cake) feeds it with
   Create's sounds; the cage, head and rods are drawn through `native_block_entity_submit` as `BlazeBurnerRenderer`
   does; a mixer's basin over a burner meets `heated` and `superheated` mixing recipes). Not yet: the flame's
   scrolling texture, goggles and hats on the head, the head's easing and the particles, capturing a blaze with the
   empty burner (using an item on an entity), the lit blaze burner (soul fire) variant, the stock keeper's heat,
   compacting recipes' heat with a press, blaze burners on contraptions, `FULL_BRIGHT` light on the head and rods.
   The clutch and gearshift (`Kind::SplitShaft`: `SplitShaftBlockEntity.getRotationSpeedModifier`,
   a powered gearshift reverses what is beyond it and a powered clutch cuts it; `powered` is their own state property,
   set from the redstone signal beside them, detached and attached again as `neighborChanged` does; their shaft
   halves drawn by `SplitShaftRenderer`'s rule), the speedometer and stressometer (`gauge.rs`: `SpeedGaugeBlockEntity.getDialTarget`,
   `StressGaugeBlockEntity`'s target, `GaugeBlockEntity`'s eighth-a-tick chase, `GaugeBlock.shouldRenderHeadOnFace`;
   `Kind::Gauge`, a block for each `axis_along_first` as the saw has, placed by `GaugeBlock.getStateForPlacement`
   through `native_state_for_placement`; their shaft, heads and dials drawn by the native mod as `GaugeRenderer`
   does; goggle tooltips). Not yet: the gauges' particles (`animateTick`), their plate-shaped collision (a 12/16
   casing stands in), the stressometer's advancements and its `GaugeObservedPacket` (nothing reads it).
   The spout (`spout.rs`, `spout_glue.rs`: `SpoutBlockEntity`, `FillingBySpout`, `GenericItemFilling`): a mod block
   entity on the enchanting table's template with `AllShapes.SPOUT`, its 1000 mB tank in its data and reached by
   pipes on every side but the bottom; over a depot or a belt it holds an item it can fill, starts a 20-tick
   process with the spouting sound and at its fifth tick fills one item (`create:filling` recipes whose fluid
   Horizon has, a glass bottle with water into a water bottle, an empty bucket into a water or lava bucket), the
   result held as `convertToAndLeaveHeld`; the tank's fluid, the stream and the nozzle's three bits squeezing as
   `SpoutRenderer` draws them. Not yet: `BlockSpoutingBehaviour` (filling blocks below: other mods' only),
   sequenced assembly's filling steps, potion and tea fluids (no fluid in Horizon), the food advancements, spouts on contraptions. Comparators read its fill (`levelOfSmartFluidTank`). Its splash (20 particles) and the falling
   particles while it pours are sent from the server; Create's `FluidParticleData` draws the fluid's texture, which
   Horizon's particles can't, so the fluid block's own particle stands in (decision: the closest Create look).
   The mechanical crafter (`crafter.rs`, `crafter_glue.rs`: `MechanicalCrafterBlock`, `MechanicalCrafterBlockEntity`,
   `RecipeGridHandler`, `MechanicalCraftingInput`, `MechanicalCraftingRecipe`): `Kind::MechanicalCrafter`, a small cog
   about its facing's axis (2 su per rpm), on a stone template with its own `facing` and `pointing` and a block entity;
   placed against another crafter it takes its facing and points back at it (`getStateForPlacement`), the wrench on
   its front cycles `pointing`. Its slot takes one item from a click on the front, funnels and chutes; a cover goes
   on an empty slot and comes off with an empty hand. With its cog turning, a chain whose crafters all hold an item
   or a cover (any item, on a redstone signal) begins: items pass along the pointing crafters (`mergeOnto`) at the
   countdown speed, the last one crafts with vanilla's crafting recipes (not `_manual_only` ones) or
   `create:mechanical_crafting` ones (mirrored or not), crafting remainders beside the result, puts it into the
   inventory or belt it points at and ejects the whole grid when there is none or no recipe; the crafter sounds play.
   Its lid, arrow or belt and the items before its face are drawn. Connected inputs (`ConnectedInputHandler`): the
   wrench near an edge of a side face toward a crafter facing the same way (`EdgeInteractionHandler`'s 3/16 strip)
   joins their groups or splits one where they meet; an item put into any crafter of a group goes one at a time into
   its crafters in Create's order (top rows first). A broken crafter (`onRemove`, seen at the level's tick end) drops
   its cover, ejects its chain's grid and splits its group where it stood. Arms take from and deposit into crafters
   (`CrafterPoint`: half a block out of the front, its group's slots). Hoppers putting items in are **blocked on the engine** (no issue):
   a mod block entity's container has no slot limit or `isItemValid` hook, so a hopper would fill the slot past one
   item and while the crafter works; the crafter keeps its slot shut to them. The grid is saved with each item's
   components (`GroupedItems.write`). The items move along the way out while exporting, gather and shrink as it crafts,
   and the result spins out (`renderItems`, the countdown told to clients). Not yet: the belt's scrolling texture, the
   connected casing (`CrafterCTBehaviour`), the firework recipe's ingredient limit, the particles, advancements.
   The crushing wheels (`crusher.rs`, `crusher_glue.rs`: `CrushingWheelBlock`, `CrushingWheelControllerBlock`,
   `CrushingWheelControllerBlockEntity`): `Kind::CrushingWheel`, a shaft along its axis (8 su per rpm), drawn turning
   from its OBJ model. Two wheels on one axis with a block between get a controller there (`updateControllers`:
   `valid` and its facing from their opposite speeds, looked at when a speed changes and every 20 ticks; it goes,
   dropping what it holds, when a wheel goes). Items in its cell, or from belts, funnels, chutes and arms
   (`CrushingWheelPoint`, deposit only), are crushed with `create:crushing` then `create:milling` recipes at the
   wheels' speed (a stack slower, by `log2` of its count; with no recipe the items are gone, as in Create), each item
   rolled, and the results go to what takes items from belts below (or along a sideways facing) or are thrown the way
   it faces. Decisions, as Horizon has no collision shape for one entity: an item in the cell goes in at once rather
   than being drawn through slowly, and results start clear of the controller's collision (Create throws them 0.55
   out with its collision empty for them); `BypassCrushingWheel` is kept for the items thrown out. Hoppers don't
   reach it (a mod container has no slot limit or validity hook: **blocked on the engine**, no issue). Mobs and survival players in
   it are pulled along and hurt (4, through `/damage`); `entityInside`'s push on a wheel's top never
   runs in Create either (its full collision keeps entities below the 1.25 it asks for). Not yet: the ambient sound and crit particles, item particles (they don't go over the network), the
   wheels' advancements.
   Encased shafts and cogwheels (`Kind::EncasedShaft`, `Kind::EncasedCog`: `EncasedShaftBlock`, `EncasedCogwheelBlock`,
   `EncasableBlock.tryEncase`): a casing used on a bare shaft or cogwheel encases it (the casing is kept); a cog takes
   a shaft on each side where a neighbour reaches it, the wrench on its axis switches one on or off and elsewhere turns
   it (its shafts swapping as `getRotatedBlockState` does), a sneaking wrench gives back the bare block. Encased cogs
   mesh as cogs and have shafts only where set; the casing's model stays in the chunk, the cog and shaft halves turn
   inside. Not yet: the casing's connected textures (`EncasedCogCTBehaviour`), `canSurvive` beside large cogs, the
   encase's particles. Generators start again
   after a change of state (`switchToBlockState`'s `reActivateSource`: a wrenched motor no longer stops for good).
   The encased chain drive and adjustable chain gearshift (`chain.rs`, `Kind::ChainDrive`, `Kind::ChainGearshift`:
   `ChainDriveBlock`, `ChainGearshiftBlockEntity`): `part` and `axis_along_first` from their neighbours (`updateShape`, at
   placement and on a neighbour's change), the speed passed along a line (`areBlocksConnected`), the gearshift's
   modifier from its redstone signal (`getModifierForSignal`, saved with the kinetics) and `powered`; the wrench turns
   them about their line and rebuilds the part (`updateAfterWrenched`). A shaft turns inside the casing. The flywheel
   (`Kind::Flywheel`: a shaft along its axis, no stress, the wheel and shaft drawn turning; not yet `visualSpeed`'s
   easing). The turntable (`Kind::Turntable`: turned from below, 4 su per rpm, its model turning): mobs on it turn at
   `speed * 3 / 10` degrees a tick (`entityInside`, through `/tp`), the window's player's view turns
   (`TurntableHandler`, a tick at a time rather than each frame, LAN guests not) and a box over its edge is carried
   round. Not yet: rotating and mirroring chains on contraptions (`transform`).
   The rotation speed controller (`Kind::SpeedController`: `SpeedControllerBlock`, `SpeedControllerBlockEntity`,
   `RotationPropagator.isLargeCogToSpeedController`): a shaft along its horizontal axis, AllShapes' shape, placed across
   a large cog above; it sets that cog's speed (`getConveyedSpeed`, `getDesiredOutputSpeed`) to its target, saved with
   the kinetics. The mouse wheel over its value box steps the target in place of Create 6's value board, as on the
   motor (so 0, which the board allows, isn't reached). Its shaft and the bracket under the cog are drawn. Not yet: its
   value's tooltip, `PlacementHelper` (a large cog placed from the controller), the advancement.
   Redstone logic (`redstone.rs`, `redstone_glue.rs`): the analog lever (`AnalogLeverBlockEntity`: a lever's template,
   its use and `native_block_signal`, 0 to 15, sneaking lowers it, its neighbours told after 15 ticks; handle and
   indicator drawn), the toggle and powered latches (`ToggleLatchBlock`, `PoweredLatchBlock`) and the pulse repeater,
   extender and timer (`BrassDiodeBlockEntity`). Decisions: a diode's input is read in its block entity's tick with its
   delay counted there, not with block ticks; `SignalGetter.getSignal` is private to `horizon-world`, so a small module
   (`level_signal.rs`) is injected there; the mouse wheel steps a brass diode's interval over the board's rows (said on the
   action bar) in place of Create 6's value board. Not yet: the wrench on diodes (turning, sneaking), the lever's easing
   and the indicators' colours (diodes' `FLEXPEATER_INDICATOR`), goggles on the lever, the particles, wires
   connecting only along a diode's axis (`canConnectRedstone`), the clipboard.
   Blocks that aren't full (the link, the blaze burner, the turntable, the diodes) stand on a chain's template, which
   hides nothing: on stone's they cut holes in the block under them.
   Glass and encased pipes and the fluid valve (`fluids.rs`, `fluid_glue.rs`, `Kind::FluidValve`): the wrench takes a
   pipe to glass along its straight axis (a corner: the axis of the side nearest the click, a click along that axis
   passing) and glass or an encased pipe back to a pipe (`updateBlockState`); copper casing on a pipe or glass makes an
   encased pipe keeping its sides, not used up; flows stay through all of these (`cacheFlows`, `loadFlows`). The valve:
   `DirectionalAxisKineticBlock` placed by shafts and pipes beside it, `getPipeAxis`, no stress, the pointer chasing open
   at a positive speed and closed at a negative one at speed / 320 a tick and standing still at none, `ENABLED` following
   it; closed it keeps its pressure and pulls nothing (`canPullFluidFrom`). Glass pipes draw their flows
   (`TransparentStraightPipeRenderer`, sent in `create:fluids` as kind 5). `PipeAttachmentModel`'s attachments are state
   properties, as the fluid pipe's are: `end_negative` and `end_positive` on glass and valves, `false`, `true` or `drain`
   on an encased pipe's sides. Decisions: a valve's pointer is not saved, it starts from `ENABLED` (Create's `Pointer`
   tag only differs for a valve left turning); a guest's wrench on a corner pipe counts as a click in the middle, as the
   native hook has no click point for them. Not yet: the casing's connected textures on encased pipes, waterlogging of
   these blocks, brackets, the `GLASS_PIPE` advancement.

   The smart fluid pipe (`fluids.rs`, `fluid_glue.rs`): placed as `SmartFluidPipeBlock.getStateForPlacement` has it,
   `FACING` turned by the wrench from above or below, open along `getPipeAxis`, its filter in a block entity's data and on
   its slot (`SmartPipeFilterSlot`), drawn there; `canPullFluidFrom` takes empty or what the filter lets through, and a
   changed filter runs `propagateChangedPipe`. Decisions: a block with `face`, `facing` and no use of its own has no
   template here, so a chain's with its own properties. Engine gap: only buckets count as holding a fluid
   (`GenericItemEmptying`), as Horizon has no other fluid items or Create's fluids, and filter items carry no lists yet
   (`Filter` keeps an item's id): any other item passes nothing, as an item with no fluid does in Create.

   The item drain (`spout.rs`, `spout_glue.rs`: `ItemDrainBlockEntity`, `ItemDrainItemHandler`,
   `GenericItemEmptying`): a mod block entity with `AllShapes.CASING_13PX` holding one item and a 1500 mB tank that
   pipes drain from every side but the top and can't fill. Items come in from belts and chutes
   (`DirectBeltInputBehaviour`), funnels and arms on its sides (the item handler; hoppers through the slot travel
   south, as no side is known), and dropped on it (travelling north, as Create's `Direction.getNearest` of no
   motion); one item is taken when it can be emptied. It moves 1/8 a tick, stops at the middle for 20 ticks while
   the tank can take the fluid, is emptied at five (`create:emptying` recipes whose fluid Horizon has, water and lava
   buckets, the water bottle), then goes on to what takes items from belts beyond, or is ejected when nothing solid
   is there. A full bucket clicked on it empties into the tank; another click takes the held item back. The tank's
   fluid and the item lying on it are drawn. A belt funnel on it (pulling, as on a depot or a saw facing up:
   `canSupportBeltFunnels`) takes the item at its edge first (`tryExportingToBeltFunnel`). Not yet: the
   item's roll and the stream over it while emptying, potion and other fluids Horizon has no block for (honey, tea,
   milk), advancements. Comparators read its fill (`levelOfSmartFluidTank`).

## Rework onto Horizon's hooks (pin 674948d)

The new pin has the hooks most workarounds here stand in for. Each wave replaces them as Create does it, then
VALIDATION.md is checked again for what it touches:

1. Done: comparators: `native_has_analog_output` / `native_analog_output` for tanks (`ComparatorUtil.levelOfSmartFluidTank`),
   the whole vault (`ItemHelper.calcRedstoneFromInventory`), depots, basins, chutes, belts' ends as Create answers.
2. Create's own sounds (`native_play_sound` takes a mod's event by name, API 61) wherever Create plays them
   (`sounds.rs`: AllSoundEvents' compound parts; done: the wrench's rotate and remove, contraptions assembling and
   disassembling, the depot's slide and pick-up, the press's clang and bonk, slime on chassis and pistons, the mixer's mix, the crank, funnel flaps, super glue's corners and removal; left: the
   saw, value scrolling, the arm's and deployer's vanilla sounds); Create's
   particles (`ParticleOptions::named`, #5482) for the fan's air flow and the others.
3. Fluids through `native_block_entity_fluids` (block entities on any template, #5549): tanks and basins keep their
   tanks in their block entity; pipes and other mods reach them through the engine.
4. Blocks with their own state properties (#5290) and `render_shape`: belts (`slope`, `part`, `casing`), funnels
   (`extracting`, `powered`), tunnels (`shape`), chutes (`shape`), fluid pipes (six sides), tanks (`top`, `bottom`,
   `shape`), the vault (`large`), the arm (`ceiling`), the smart chute (`powered`); rotating blocks drawn by their
   block entity (`ENTITYBLOCK_ANIMATED`) instead of blank blockstates; shapes by several properties and
   `collision_shapes`.
5. Placement and shape updates (kinetic blocks done, see the engine gaps): `native_state_for_placement` (`getStateForPlacement`), `native_update_shape`,
   `native_can_survive`, `native_block_entity_inside` / `native_block_fall_on` for belts, funnels and chutes.
6. Drawing through `native_block_entity_submit` instead of the `transient_block_geometry` mixin where a block entity
   draws; the value settings board as a mod screen (`native_mod_screen_open`, API 111).

## Outlines

Catnip's `Outliner` (`outline.rs`: slots kept alive each tick, eight ticks of fading, `AABBOutline`'s edges as thin
boxes) is drawn by the native mod in the frame's own pass with a white sprite (`create:block/hz_blank`, made by
`prepare_content.py`; Create's is ponder's `special/blank`). The server tells a player's client what to show over
`create:outlines` (O1: slot, box, colour, line width, ticks). Fading outlines are drawn whole until they go (the frame
takes no alpha). Used by: the large water wheel's `showBounds`, the arm's selection.

## Engine gaps to raise upstream (as hook point requests)

Upstream issues for these gaps (horizon-rust/horizon), checked 2026-11-03. Closed after the old pin `aa27eca`, so
usable now that the pin moved (see the decision below): #5290 (a mod block declares its own state properties: belts, pipes, funnels, tunnels,
tanks, the vault's `LARGE`, the arm's `ceiling`), #5549 (block entities for mod blocks whatever their template),
#5497 (a mod block's comparator output), #5482 and #5483 (mod particle types), #6138 (random ticks on any mod
block), #1040 (mod capability: storage). Still open: #5548 (drawing baked models at a block every frame),
#5550 (render shape, composed shapes, a state chosen on placement), #4169 (connected textures), #6301 and #5553
(mod fluids), #5556 (fake players, mod entity types, moving groups of blocks), #5212 and #6300 (mod entity and
block types from code), #7465 (a mod particle's own behaviour). None found for mod sound events.

Decision (2026-11-03, revised the same day at the user's request): the pin moved from `aa27eca` to Horizon main
`674948d` (2026-11-03) and horizon-mixins `ddd9700`; every mixin applied unchanged (one API change: `Ingredient` is a
struct, `.order` for its listed items). The workarounds the closed issues above replace are reworked onto the new
hooks before the remaining "Not yet" items are ported.


- A mod-drawn screen outside a container menu, with a free cursor: Create 6's value settings board (hold right
  click on a value box, pick a value with the mouse). The pin has `native_screen`, `native_screen_draw` and the
  `native_screen_*` input hooks (found 2026-11-04, not used yet); until a board is built on them the value boxes take
  the mouse wheel (`native_wheel`, as Create 0.5 did).
- Found on the pin 2026-11-04 and not yet used: `native_interact_entity` (an item used on an entity: feeding,
  capturing a blaze), `native_block_entity_fluids` and `native_item_fluid_tank` (fluids in block entities and items),
  `native_block_entity_energy`, `native_can_survive`, `native_may_place_on`, `native_neighbor_changed`,
  `native_tick_block`, `native_random_tick`, `native_block_highlight`, `native_emissive`. Used since: `native_block_signal`
  and `native_is_signal_source` (the redstone link).
- Fixed on the new pin: `native_state_for_placement` (`placement.rs`) places shafts, cogwheels, large cogwheels,
  gearboxes, creative motors, hand cranks, presses, fans, water wheels, drills, bearings, rope pulleys, pumps, the
  redstone contact, the portable storage interface, harvesters, ploughs, the arm and the vault as Create's
  `getStateForPlacement` does (shafts beside them, the placer's look, sneaking, the clicked face, chutes for the
  fan, pipes for the pump), replacing the fixes made a tick after placement. Still turned after placement until
  their blocks take their own properties: pistons, deployers and saws (`axis_along_first` is a block each), gantry
  carriages, chassis, chutes and funnels.
- Basins with a `facing` (pouring sideways onto belts and depots) need a template with a hopper's facing and a
  block entity; none has both. Basin and depot shapes stay the enchanting table's (12/16 high), so items rest on
  top instead of inside.
- Container insertion rules: hoppers fill a basin's input slots as any container's, not as `BasinInventory`
  (one slot per item kind).
- Pushing players from a native mod's server tick (the fan's current): `native_push_mob` reaches mobs only; the
  window's own player is pushed from `native_tick`, guests on a LAN server not at all.
- Mod particle types and mod sound events: native mods spawn vanilla particles and play vanilla sounds only.

- Draw hook for mod block entities (today: a mixin into `transient_block_geometry`), and a per-state "hide from
  the chunk mesh" flag (today: blank blockstate models plus hidden `hz_*` blocks holding the real models).
- Block templates with arbitrary property sets (encased cogs, belts, funnels have no vanilla template).
- Sneaking and the clicked hit position in `native_right_click_block` (Create's crank turns backwards when
  sneaking; value boxes need the hit position).
- Custom entities with block collision and rendering (contraptions, trains).
- **Recipes in subfolders don't load** (`Recipes::load` reads `data/<ns>/recipe/*.json` only; vanilla reads the
  folder recursively): 1198 of Create's recipes, crafting included. Worked around by flattening them
  (`pressing/iron_ingot` → `pressing.iron_ingot`) in `prepare_content.py`.
- `native_recipes_of_type` and `native_recipe_ingredient` answer only inside a player's hook: a block entity's tick
  gets nothing. Worked around by reading `Game::recipes()` directly.
- Block shapes can only be keyed by a template property: a template with none (the enchanting table that hosts the
  depot and millstone block entities) keeps its own shape.
- No template has a belt's properties (`facing`, `slope`, `part`, `casing`): a block per part on a grindstone's
  template, its `face` holding the slope; vertical and sideways belts and belt casings need more. Belt shapes
  follow `face` alone (no `facing`): sloped belts are a lower half block. Funnels have the same problem
  (`extracting`, `powered`): a block per `extracting` on a lightning rod's template, and belt funnels have no
  block (a horizontal funnel over a belt acts as one, with the funnel's model).
- Tunnels have the same problem (`horizontal_axis`, `shape`): a block per shape on a chain's template, the shape and
  axis set by the native mod a tick after placement or a neighbour's change (no `getStateForPlacement` or
  `updateShape` for mod blocks), and collision shapes by axis only.
- Mod block entities only on templates that have one: belts, funnels and tunnels have none, so their items and timers live
  beside the world in the native mod (only an item's id, count and damage are saved), and hoppers can't put
  items onto belts.
- No native hook to take items out of a container (`native_insert_item` puts them in, and can't simulate):
  funnels and chutes take from chests and barrels (Horizon's `BlockEntity::Chest`), mod block entities and belts
  only, with no comparator update; a chute's "will it fit" simulation into a container is hopeful.
- No redstone signal query from a native mod: funnels are never paused by redstone.
- No `entityInside` / `updateEntityAfterFallOn` hook for mod blocks: belts, funnels and chutes look for item
  entities each tick.
- Mod containers can't say which stack a slot takes: hoppers stack onto a chute's item (`ChuteItemHandler` takes
  only into an empty chute).
- **No shared item, fluid or energy handlers between mods.** NeoForge's capabilities (`Capabilities.Item.BLOCK`,
  fluids, energy) are how Create talks to Mekanism, AE2 and others. Horizon has only a block entity's fixed
  container slots (hoppers and other mods can use them: this is how the depot, millstone and basin are reached
  today); fluids and energy have nothing. Horizon's `docs/native-hooks-neoforge.md` proposes a
  `block_capability(pos, kind, side, out)` hook, not built as of `aa27eca`. Plan: keep each machine's inventory
  and tank access behind one small trait in the port, so moving to that hook (or contributing it upstream) touches
  one place (items: `ItemHandler` in `arms.rs`, which the arm uses for every point and funnels and chutes for vaults
  and deployers; the older belt, funnel and chute code still has its own helpers in `logistics.rs`); until then, a
  shared registry module with the proposal's signature is the fallback if another ported mod (Mekanism, AE2) must
  reach Create's fluids. Fluids do this now: pipes reach every fluid holder only
  through `create_fluids::FluidAccess` (`has_handler`, `handler_tanks`, `fill`, `drain`, `drain_any` at a block
  face), which `fluid_glue::Access` answers for tanks and basins; a capability hook would answer it instead.
- horizon-port-pack leaves out the blocks no template fits (the andesite and brass funnels and tunnels, fluid pipe
  and tank, item vault, mechanical arm, deployer...) and drops their recipes with them (`recipes_unregistered` in its carry report):
  `prepare_content.py` declares those blocks itself and, given `--jar`, carries the crafting recipes of the ones it
  declares from the jar (`take_left_out_recipes`, ingredients upgraded to 26.3's strings). Recipes of blocks not
  ported yet stay out.
- NeoForge's convention tags (`c:stripped_logs`, `c:cobblestones`...) aren't in Horizon: `prepare_content.py` adds
  NeoForge 21.1's values for vanilla's items (`CONVENTION_TAGS`); tags only other mods fill stay empty.
- Horizon's client doesn't run native mods, so a click on a block whose template has no use of its own also sends
  the off hand's use (Create's client answers SUCCESS and sends none): `glue::right_click` drops it.
- Fixed on the new pin: fluid pipes take their six sides as state properties (#5290), placed and turned through
  `native_state_for_placement` and `native_update_shape`. Their shape table lists every combination of the six sides'
  five values (15625 entries, an 11 MB block file): shapes can only be keyed by whole property tuples.
- Fluids exist in Horizon only as water and lava blocks: there is no fluid registry a mod can add to, so Create's
  fluids (honey, chocolate, tea, milk via NeoForge, potions) are at most names in a tank (honey drained from a
  hive), can't be poured, have no textures to draw by, and the recipes that use or make them are skipped. Fluid
  tags (`c:milk`) have nothing to name either.
- Mod blocks can't take a `UseItem` back: Create's client answers SUCCESS to a click its block took and sends no
  `UseItem`; Horizon's client sends it anyway, so `glue::use_item` drops the main hand's use in the tick of a click a
  Create block took (one global note, not per player).
- No connected textures for chunk-meshed mod blocks (Create's `CTModel` needs the model per position): `ct.rs` has
  the rules, and only blocks the native mod draws itself use them (tanks, vaults); casings, chassis and windows show
  each block's own frame.
- The fluid tank's template (glass) has no block entity: its contents live in the native mod beside the world
  (lost if the native mod is removed), hoppers and comparators see nothing. Drawn fluids are opaque (the native
  draw hook takes vertices on one layer) and water takes the plains' colour, not the biome's.
- No template has a facing and a block entity without uses of its own (a dispenser's would dispense a mod's
  items on redstone): the deployer keeps its state beside the world and hoppers can't reach it; nor six facings and
  `axis_along_first` (a block per value). 
- Native mods can't act as a player (`DeployerFakePlayer`): the deployer places blocks through Horizon's own
  placement code and breaks them through the level's, which a module inside `crate::game` reaches; items' uses,
  blocks' uses and entities need a fake-player hook.
- `native_place_block` gets no clicked face (the vault takes a neighbour's axis only when there is one vault beside
  it) and can't change the world (placements are finished at the level's tick); `native_right_click_block` has no hit
  position (the deployer's front stands for Create's `location.length() > .75`).
- No sound hook on `Game` for a native mod's block entity tick or level tick (`native_play_sound` is on the level's
  hooks): the arm's pick-up and the deployer's sounds aren't played.
- Loot tables a native port writes itself must be in 26.3's format (`"condition": {"type": ...}` per pool), as
  `horizon-port-pack` upgrades carried ones; 1.21.1's `conditions` list drops nothing. And a template's required
  tool applies to every block made from it: extra blocks (belt parts, the collecting funnel) need their base
  block's `mineable/*` tags or they drop nothing (`prepare_content.share_block_tags`).
