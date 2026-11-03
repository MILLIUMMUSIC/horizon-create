# Validation

## Horizon pin moved to 674948d (macOS, offscreen)

The port builds against Horizon main `674948d` (2026-11-03) and horizon-mixins `ddd9700` with every mixin applied
unchanged; the world with the earlier rigs loads and draws (tanks, vault, arm, belts, chutes).

Comparators (`native_analog_output`): a tank with 4 buckets of 8 powers the wire behind its comparator to 8, and a
depot holding 32 cobblestone to 8. Not checked yet: the vault (whole multiblock), the basin's inputs.

Create's sounds: the client hears `create:wrench_rotate` when a shaft is wrenched round and `create:slime_added`
when a slimeball makes a mechanical piston sticky (it turns into a sticky one, a slimeball used up in survival).
Not checked yet: the contraption, depot and press sounds.

Moving flaps: an andesite funnel set on a chest's side (`setblock`) gets its block entity, counts its swings as it
puts cobblestone out in front (`Flapsouth` 1, then 5 as the items were cleared) and its flap is drawn swung out,
moving between frames. Not checked yet: tunnels' flaps on a belt, a collecting funnel's flap swinging in.

Ceiling arms: a mechanical arm placed against the underside of a stone block hangs from it (`ceiling=true`),
drawn upside down; with a depot to take from and one to deposit to (cogwheel and creative motor beside it), it
moved 8 iron ingots from the one to the other.

Fluid pipes on their own properties: a pipe placed by a tank opens towards it with a drain, a second placed on top
turns the first into a junction once a third goes beside it (casing drawn), open ends have rims and joined pipes
join without them, as Create's `PipeAttachmentModel` draws them. Lava from a creative tank runs through a pipe that
bends at a pump (creative motor and cogwheel, facing east) into another tank: 2328 mB within ten seconds, the
wire behind the second tank's comparator at 4.

The vault on stone's template with its own `axis` and `large`: a vault placed against a vault's side takes its
axis and joins it (`Length: 2`), one placed on the floor takes the placer's facing's axis, and a hopper above fills
it (5 diamonds in its block entity). A bearing placed beside a shaft faces away from it.

Placement (`native_state_for_placement`): a shaft placed on the floor beside a shaft along X lies along X, one with
nothing beside it takes the look's axis (Z, looking north), and a fan placed beside the first shaft blows away from
it (east). The checks below this
section were made on the earlier pin `aa27eca`; they haven't all been repeated.

## Chute goggles, survival and the powered smart chute (macOS, offscreen)

Seen working: wearing the goggles, a chute looked at shows "Chute Information", "No attached fans" and "-> Items
move Downward"; a smart chute beside a redstone block is drawn with its powered model; a chute under a chute set
to face east breaks and drops (the upper one then turns straight, having no chute below beside it).

Not checked yet: the fan lines and "Contains" in the goggles, a powered smart chute losing its signal.

## Belt covers and riders (macOS, offscreen)

Seen working: a belt along X (seven blocks, a creative motor at 16 RPM) with three segments cased in andesite and
stone on one of them: the cover's frame drawn under the stone. A cow and a pig (no AI) put on it, the pig behind: the
cow stops at the belt's end (x 40.0) and the pig stops behind it at x 41.0 (half a block from the cow's box).

Not checked yet: brass covers, a water bottle washing a dyed belt, the window's player stopped behind a mob.

## Arm selection outlines (macOS, offscreen)

Seen working: with the mechanical arm item held, two depots clicked are outlined, one blue (extract), one yellow
(deposit); a left click on the blue one takes it out of the selection (its outline goes, the depot stays).

Engineer's goggles used on a placed arm: the arm's data reads `Goggles: 1b` and the goggles stay in hand.

Not checked yet: the wrench showing a placed arm's points, the outlines going when the item is put away, the
goggles drawn on the claw (the screenshots only showed the arm from behind).

## Large water wheel placement (macOS, offscreen)

Seen working: two large wheels along X side by side drawn (the second with the extension's longer axle); a large
wheel clicked onto the stone floor with room around it is placed with its structure; one clicked where a stone
block stands in its ring is not placed, the item stays in hand and "Clear Blocks for Placement" shows in red.

The space a blocked wheel needs is outlined in red (3×3 across its axis) for a moment.

Not checked yet: the extension's axle seen closely against the plain one, a wheel placed against a wall's side.

## The wrench on minecarts (macOS, offscreen)

Seen working: in survival, one left click with the wrench on a minecart on a rail breaks it and drops it as an item;
a bare hand's click leaves it. Not checked yet: a guest on a LAN server (`native_attack_entity`), creative (the
vanilla hit breaks it anyway).

## Fans pushing the player (macOS, offscreen)

Seen working: a creative motor (16 RPM) driving an encased fan facing west; the player in survival standing three
blocks in front of it is pushed west (velocity about −0.009 a tick, about 0.6 blocks in three seconds on stone).

Not checked yet: sneaking (an eighth as hard), creative flight (not pushed), a nozzle pushing the player, a fan
blowing up lifting the player.

## Fluid tank multiblocks (macOS, offscreen)

Seen working: twelve tanks placed by `setblock` as a 2×2 base three high join into one tank (the goggles show
"Capacity: 96000mB"), drawn with Create's models for each block's place (a window across the seam of each 2-wide
face, caps at the top and bottom); two stacked tanks join as a 1×1×2 tank (16000 mB). Water buckets (creative) fill
the big tank and the water is drawn through its windows as one box as high as it is full; the contents are kept
over a restart (`create_fluids_*.txt`, new `tank` lines). The wrench toggles a whole tank's windows off. Removing a
block of the top layer splits the tank: the lower 2×2×2 forms again with the water, the three top blocks stay single
tanks. The creative fluid tank (its own purple models) takes a lava bucket's fluid for good; in survival an empty
bucket takes a lava bucket out once and empties it, and a water bucket on a regular tank pours beside it instead
(Create: "in Survival Mode Fluids cannot be added or taken manually"). A bucket's click on a tank no longer also
pours a source next to it (the client's `UseItem` after a click Create took is dropped).

Connected textures (`ct.rs`): a 1×1×2 tank shows one frame over both blocks; a 3×3×6 item vault placed by
`setblock` draws as one vault with the large sheets (frame and rivets running across its blocks), each face lit by
the block it faces.

Not checked yet: 3-wide tanks, a 2-wide vault (medium sheets), vaults along X, vaults and tanks on moving
structures (drawn plain), two tanks of different fluids side by side staying apart, pipes and pumps on a
multiblock tank (they reach the controller's tank through any block), a tank 32 high.

## Goggles on tanks (macOS, offscreen)

Seen working: a fluid tank filled with a water bucket, looked at with the goggles, shows "Fluid Container",
"Water" and "1000mB / 8000mB" beside the crosshair.

Not checked yet: an empty tank's capacity line, a basin's fluids, Create's own fluid names.

## Deployers punching mobs (macOS, offscreen)

Seen working: a deployer facing east (16 RPM) switched to punch with the wrench hits a pig in the block two in
front once per cycle (10 health down by 1 each time) until it dies and drops its porkchops; with an iron sword
handed to it, a pig takes 6 a hit and dies in two. 181 unit tests pass (2 new).

Not checked yet: armour, other weapons, several mobs in the block.

## Scrolling a creative motor no longer breaks what it drives (macOS, offscreen)

Seen working: a creative motor driving a shaft and an encased fan, scrolled 40 steps down and 60 up with the mouse
wheel, keeps them (the shaft used to break): the value goes to the server once the wheel rests (half a second),
not at every step, which counted towards Create's flicker score (`MAX_FLICKER_SCORE`). Before, a quick scroll
broke shafts, fans and pistons in its network.

## Item vaults as contraption storage (macOS, offscreen)

Seen working: a mechanical piston (16 RPM) pushing a slime block with an item vault on it and a drill east: the
drill breaks the stone in its way and the cobblestone goes into the vault, still there when the vault is put
back at the end. (The first try dropped it: Horizon's saved data leaves out trailing empty slots, so a captured
empty vault had none; they are filled back to its container's count now.)

Not checked yet: a portable storage interface emptying a vault on a structure, a vault of several blocks.

## Tunnel and funnel flaps (macOS, offscreen)

Seen working: an andesite tunnel on a belt shows its four flaps hanging in its openings along the belt; a
horizontal brass funnel on a chest's side shows its flaps. Content only (blockstates), no native change.

Not checked yet: the T and cross shapes' side flaps, brass tunnels, andesite funnels.

## Vertical and sideways belts (macOS, offscreen)

Seen working: the connector between two shafts along X one above the other (four apart) makes a vertical belt
(start, middles, end), drawn upright with its pulleys at both ends; between two upright shafts four apart it
makes a sideways belt, drawn standing on its edge. 179 unit tests pass.

Not checked yet: their speed passed on and scrolling, breaking them, casings or the wrench on them, a pulley in
the middle of one.

## Riding belts (macOS, offscreen)

Seen working: on a belt turned by a creative motor at 16 RPM, a pig and the player standing on it move along
it, about two blocks in three seconds (`speed / 480` a tick), the way its items go. 179 unit tests pass (1 new).

Not checked yet: sloped belts, the end of a belt (into a wall), centring from a side, sneaking, a walking player.

## Extending belts with the connector (macOS, offscreen)

Seen working: the connector on the outer half of one belt's end joins it to the belt beyond (one five-block
belt), and on the joined belt's end adds a sixth segment; driven by a creative motor, its cobblestone and
potion (one from each former belt) run along all of it and are thrown off its end. 178 unit tests pass (1 new).

Not checked yet: joining a belt running the other way (flipped), sloped belts, the colour taken over, the shafts
and connector given back in survival, the 20-block limit.

## Belt casings and the wrench (macOS, offscreen)

Seen working: brass and andesite casing clicked on two segments of a six-block belt draw Create's cased models
(brass and andesite sprites); the wrench on the andesite one's side takes the casing off. The wrench on a middle
segment's top splits the belt in two (end and start parts beside each other, the items staying on their halves);
on the outer half of the end segment it takes that segment off and the one before becomes the end (the belt
stands). 177 unit tests pass (1 new).

Not checked yet: shortening at the start (items moved back), a survival split's cost, a pulley turned back,
casings on sloped belts, items thrown off a shortened end, a LAN guest's wrench (the look ray fallback).

## Belt dyeing, stacked and upright items (macOS, offscreen)

Seen working: blue dye clicked on a six-block belt turns all of it blue (its scrolling sprite swapped); a water
bucket washes it back (the bucket stays full, no water is placed). On the stopped belt a stack of 16 cobblestone
from a chute lies as a pile of cubes spread a little, and a potion stands upright. 176 unit tests pass (2 new).

Not checked yet: the colour after reloading and on a sloped belt, a dyed belt broken in two, upright items on a
slope.

## Smart chute (macOS, offscreen)

Seen working: a smart chute between two chests, its filter set to an iron ingot by clicking its side, takes the
ten iron ingots and leaves the cobblestone in the chest above; the filter is drawn on its side. A redstone block
beside it stops it (five more ingots stay above) until the block goes. The mouse wheel over its side sets the
amount (kept in its data); at 1 the ingots come down one at a time. 174 unit tests pass (1 new).

Not checked yet: a smart chute fed by another chute or a funnel past its filter, blown up by a fan, the amount
after reloading, an empty hand clearing the filter.

## Diagonal chutes and chute shapes (macOS, offscreen)

Seen working: a hopper over a straight chute over a diagonal chute facing east hands iron and gold ingots through
it into the chute below and beside it and on into a chest; the lower chute turns into an intersection and the
diagonal one into a diagonal intersection (nothing feeds it from the side), drawn from Create's models. A chute
placed by hand on a block's east side faces east and stays diagonal. The wrench turns a straight chute into a
window chute and back from encased; industrial iron encases it; items still pass after each change. 173 unit
tests pass (1 new).

Not checked yet: a run of diagonal chutes (the plain diagonal shape), an item kept while its chute changes shape,
sneaking placement, fans through diagonal chutes, items drawn in a window, arms and funnels on a window or
intersection chute.

## Upward chutes pulling from below (macOS, offscreen)

Seen working: an encased fan facing down on two chutes, turned at 256 RPM by a creative motor, draws oak planks
dropped on the ground two blocks under the chutes into them; they rise and wait in the top chute under the fan
(the fan counts as a full block above, as in Create). Slower speeds don't reach the ground (the pull distance).

Not checked yet: items taken from a belt under an upward chute, the pull distance at 32 and 64 RPM.

## Fans through chutes (macOS, offscreen)

Seen working: an encased fan facing up under three chutes (16 RPM from a creative motor) blows iron ingots fed in
by a hopper beside the lowest chute up the column and into a chest on top; with the chest gone, gold ingots are
blown out of the top as item entities. 172 unit tests pass (1 new).

Not checked yet: a fan on top facing down pulling, an input chute above taking items, a funnel facing up above,
an inventory under an upward chute giving items, faster fans.

## Arm selection modes and new points (macOS, offscreen)

Seen working: the mouse wheel over a mechanical arm's side steps its mode (the action bar says Forced Round Robin,
then Prefer First Target); an arm set to take from a depot and deposit into a composter puts wheat seeds into it
(the composter's level rises).

Not checked yet: the modes' effect on several targets, campfires, respawn anchors, bone meal taken out of a full
composter.

## Funnel clicks and amounts, tunnels' item handler (macOS, offscreen)

Seen working: cobblestone clicked into a collecting andesite funnel on a chest goes into the chest (the whole
stack in hand); the mouse wheel over a vertical brass funnel's side steps its amount down to 1, and it then pulls
one item at a time out of the chest below.

Not checked yet: a funnel or arm inserting through an andesite tunnel onto its belt, a brass tunnel taking a
stack to distribute from a funnel, the amount after reloading.

## Large water wheel (macOS, offscreen)

Seen working: a large water wheel set about Z gets its eight structure blocks (the sides facing it, the corners
facing the side beside them) and is drawn from its OBJ model with a shaft through it; cherry planks clicked on a
structure block make the whole wheel cherry; a structure block taken away breaks the wheel, which drops itself and
takes the rest of its structure. 171 unit tests pass (3 new).

Not checked yet: water turning it (the flow two blocks out), placing it by hand, a blocked placement, saving.

## Nozzle, crafting remainders (macOS, offscreen)

Seen working: a nozzle on an encased fan facing up (16 RPM) pushes a pig beside it away, out to the edge of its
reach. 169 unit tests pass (1 new).

Not checked yet: a pulling nozzle drawing things in, items pushed, crafting remainders (no recipe of Create's own
takes an item with one).

## Fan on entities, water wheel woods (macOS, offscreen)

Seen working: a pig in an encased fan's current through a lit campfire (smoking) is pushed along, set on fire and
dies, dropping a cooked porkchop. Cherry planks clicked on a water wheel make it a cherry wheel (pink paddles,
cherry log ends); the planks aren't used up. 168 unit tests pass (2 new).

Not checked yet: blasting, splashing on an enderman, haunting a horse, a player in the current, the other woods,
the wheel's wood after reloading.

## Brass tunnel modes (unit-tested only)

`tunnel.rs` has all seven of `BrassTunnelBlockEntity.SelectionMode` (Forced split and Forced round robin give
nothing unless every target takes it, Prefer nearest always starts at its own belt, Randomize at a random one,
Synchronize holds its group's stacks until every tunnel of it has one); the wrench steps through them. 166 unit
tests pass (1 new), `horizon.py check` passes.

Not checked yet: any of the five new modes in the game.

## Parts drawn on moving structures (macOS, offscreen)

Seen working: a piston pushing a slime block with a shaft, a cogwheel, a drill and a harvester: the shaft's and
cogwheel's models are drawn while it moves (they were invisible before), the harvester's blade spins, the drill's
head is drawn.

Not checked yet: the drill's head spinning seen close, bearings (parts drawn still there), pulleys and gantries.

## Elevator clicks and redstone contacts (macOS, offscreen)

Seen working: two redstone contacts placed facing each other both give a signal, and stop when one is taken
away. Clicking the elevator pulley with an empty hand takes the elevator apart (a call does nothing then);
clicking again assembles it, and calls work again, up and down.

## Directional placement (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`. Drills, bearings, creative motors, hand cranks, encased fans,
redstone contacts and portable storage interfaces are placed by their own `getStateForPlacement`
(`DirectionalKineticBlock`: a neighbour's shaft decides, else the way the player looks; the clicked face is
read from the lightning rod template's facing).

Seen working: a drill placed by hand beside a shaft faces away from it; one placed on its own faces the player.

Not checked yet: the other blocks, sneaking, a fan beside a chute.

## Redstone locks (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`. 165 unit tests pass, clippy has no warnings. A neighbour's signal
(`hasNeighborSignal`, read through Horizon's level) now pauses funnels, locks arms and deployers, and stops
portable storage interfaces.

Seen working: an andesite funnel on a chest of 5 cobblestone, a redstone block beside it: nothing is pulled out;
the redstone block taken away, the funnel empties the chest.

Not checked yet: an arm and a deployer locked by a signal, a powered stationary interface ignored by a passing one.

## Milestone 5, part 3: elevator pulley (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 165 unit tests pass (1 new
in `contraption.rs`), clippy has no warnings.

Seen working, with an elevator pulley at y 112 turned by a creative motor (16 RPM), a cabin of a slime block with a
redstone contact facing east at y 101, and redstone contacts facing west at y 101 and 107 in the column beside it:

- When the motor starts, the column's contacts turn into elevator contacts.
- A redstone block beside the upper contact calls the cabin: it rises (drawn moving), slows down, settles at
  y 107 and is put back as blocks; the upper contact then gives a signal. Called back down, the upper contact
  stops and the lower one gives the signal.
- A player standing on the cabin rides it up and down and stays on top when it stops (four trips).
- After closing and reopening the world, the column is gathered again and calls still work.

Not checked yet: placing the pulley and contacts by hand, a cabin of several blocks, several floors, a button on a
contact, mobs riding, saving mid-trip, crafting.

## Milestone 5, part 4c: storage, deployer and portable storage interface (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 164 unit tests pass (2 new
in `actors.rs`), clippy has no warnings.

Seen working, with a mechanical piston pushing two slime blocks east at y 104 (16 RPM), a chest with 10 cobblestone
on the first, a deployer facing down on the second, a drill in front and a portable storage interface facing north
beside the second; a stone ahead of the drill, a stationary interface facing south two blocks north of the path with
an andesite funnel behind it:

- The deployer takes cobblestone from the chest and places it below the path, one block at each step.
- The drill breaks the stone; its cobblestone goes into the chest.
- The moving interface stops at the stationary one; the funnel pulls the chest's cobblestone out through it (8:
  10 - 3 placed + 1 drilled) and the chest on the structure ends empty; then the structure moves on.

Not checked yet: harvesters and saws filling the storage, the deployer punching, barrels and shulker boxes, saving
mid-move with a deployer, placing an interface by hand, an arm or chute at a stationary interface.

## Milestone 5, part 4b: mechanical saw (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 162 unit tests pass (5 new
in `saw.rs`, 1 in `actors.rs`), clippy has no warnings.

Seen working, with creative motors at 16 RPM:

- A standing saw facing east in front of a four-log oak with leaves (one leaf a step further out): it cuts the lowest
  log and the whole tree falls, leaves included, the logs and saplings dropping beside it. The blade is drawn
  sticking out of its front, textured as the stonecutter's.
- A mechanical piston pushing a saw east into a column of three logs and a leaf: the piston stops at the column,
  the saw cuts the bottom log, the column and the leaf fall, and the piston runs on to its end. Pulled back, the
  moving saw is drawn with its blade.

Not checked yet: placing a saw by hand (`axis_along_first`), a saw facing up, saws on bearings, gantries and pulleys,
bamboo, cane, cactus and chorus, crafting it.

## Milestone 5, part 4a: drill, harvester and plough (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 156 unit tests pass (11 new
in `actors.rs`, 1 in `contraption.rs`), clippy has no warnings.

Seen working, with creative motors at 16 RPM:

- A standing drill facing a stone breaks it (about 5 s) and drops cobblestone; its head is drawn turning.
- A mechanical piston pushing a drill east into a stone and a column of two gravel: the piston stops in front of
  the stone until it breaks, then breaks the top gravel first and the lower one after, and runs to its end; the
  drops lie where the drill was; the moving drill is drawn with its head.
- A piston pushing a harvester over ripe wheat, half-grown wheat and ripe carrots: the ripe ones are cut back to
  age 0 and their drops fall, the half-grown wheat is left.
- A piston pushing a plough over grass blocks with short grass on the first: the grass plant breaks (seeds drop),
  the free grass blocks turn into farmland (later dirt under the stopped piston and plough, as vanilla does).
- A mechanical bearing (slime arm, drill at its end facing the way it turns) breaks both stones in its path; a drill
  facing away from its motion leaves them, as in Create.

Not checked yet: actors on rope pulleys and gantries, placing a harvester or plough by hand, the cracks' stages
seen on blocks, saving mid-break, crafting them.

## Milestone 5, part 3: gantries (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`. 144 unit tests pass (2 new in `contraption.rs`), clippy has no
warnings.

Seen working: five gantry shafts facing east (start, three middles, end) with a carriage facing up on the middle
one, a slime block on the carriage and a stone beside the slime. A creative motor placed at the west end (+16
RPM): the carriage with the slime and the stone runs west (drawn moving) and stops on the start shaft. The motor
moved to the east end (-16 RPM): it runs east and stops on the end shaft. The shaft turns, drawn from its parts'
models; the carriage shows its wheels. A carriage whose load would hit a block (here wool above the slime) doesn't
start.

Not checked yet: placing shafts and carriages by hand (facing, parts, the carriage's axis), a player riding, saving
mid-move, glue and chassis on a gantry, vertical shafts, crafting them.

## Milestone 5, part 3: rope pulley (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`. 142 unit tests pass (2 new in `contraption.rs`), clippy has no
warnings.

Seen working, with a rope pulley at y 105 (axis X) driven by a creative motor at 16 RPM, a stone and a slime block
on the platform at y 101:

- Turned to let down, the bare rope runs down (the magnet and rope drawn moving) until it reaches the stone, and
  stops there as two rope blocks and a magnet; it doesn't take the stone, which can't go further down.
- Turned the other way, the rope is taken in and the stone, the slime and the platform block stuck under the slime
  are pulled up, drawn moving, and put back just under the pulley.
- Turned up with no rope out, it doesn't start.
- Placed with `/setblock` facing the other way round, the speed's sign decides the direction as in Create.

Not checked yet: a player riding the load, glue and chassis on the load, saving mid-move, the pulley broken mid-move,
placing it by hand (the axis rule), the coil's scrolling, crafting it.

## Milestone 5, part 2b: mechanical and windmill bearings (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 140 unit tests pass (4 new
in `contraption.rs`), clippy has no warnings.

Seen working:

- A mechanical bearing facing up on a creative motor at 16 RPM, a slime block on it with a stone, a log and
  stairs beside: the structure turns about the anchor (4.8° a tick), drawn turned; stopping the motor at 9.6° puts
  it back as it was; at 105.6° (then 91.2°) it goes back a quarter turn round: the stone from east to north, the
  log's axis from X to Z, the stairs from south to east. Seen from above, the drawn blocks turn the same way.
- A player standing on the turning stone goes round with it (radius 1, about 96° a second) and stays on top.
- A bearing facing north (axis Z) turning at -16 RPM, clicked with an empty hand at about -90°: the stone east of
  the slime goes below it, the planks above go east, the log beside stands up (axis Y), as drawn.
- A windmill bearing with a radial chassis and 20 white wool: an empty-hand click starts it at 2 RPM (20 sails / 8),
  turning the shafts beside it; clicked again it puts the 20 wool back as built.
- A turning bearing is saved when the world closes and goes on turning after loading (its speed is saved, so
  loading doesn't count as a change).
- A bearing set turning before anything stands in front of it doesn't assemble until clicked (Create assembles
  only on a speed change or a click).

Not checked yet: riders on mobs and items, a turning structure's chests and torches, glue and chassis ranges
turned with it, the bearing and sail models in every facing, crafting them, a windmill with too few sails
(`notEnoughSails` is unit-tested), a stressed network stopping a bearing.

## Milestone 5, part 2a: chassis and super glue (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 136 unit tests pass (6 new
in `contraption.rs`), clippy has no warnings.

Seen working, with the sticky piston rig of part 1 (facing up, one pole):

- A linear chassis on the X axis, both ends slimed, with a plank to the west and three stones and a cobblestone to
  the east: the piston lifts the whole row; after scrolling its range to 3 with the wrench, the cobblestone (four
  blocks away) stays behind.
- A radial chassis on the Y axis, slimed on its east side, with a 3×3 disc of stone east of it and a plank west:
  the disc rises (radius 3 takes its corners), the plank stays.
- Slime ball on a chassis' south side makes it sticky there; again on that sticky side makes all four sticky; the
  slime ball isn't used up.
- A linear chassis placed against a stone's east face while the player looks north stands on the Z axis; slimed
  on its south end, then a second chassis placed south of it: the first loses that slime (`updateShape`).
- Wrench and wheel over a chassis: "Range of Sticky Sides: 6" on the action bar, Ctrl and the wheel: 7 for both
  chassis of the group.
- Super glue: a right click on a stone ("First position selected"), another on the stone east of it ("Applying
  Glue..."): extending the piston takes both, a third stone east of them stays; the glue's box, drawn as green
  edges while glue is held, moves with them. A left click on the glued stone with glue takes the glue off (the
  edges go, the stone isn't broken).
- Glue and ranges are saved beside the world and back after loading: the reloaded glue pulls both stones back
  down; a stale range where no chassis stands any more is dropped.

Not checked yet: secondary linear chassis, chassis groups moved by one push, sneaking with an empty hand to take
slime off, glue used up in survival, glue too far or not connected, chassis on a horizontal piston, crafting the
chassis and glue, the chassis' models in every axis.

## Milestone 5, part 1: mechanical pistons (macOS, offscreen)

Built on the Mac with `tools/horizon.py build`, the content mod from Create 6.0.10's jar. 130 unit tests pass (7 new
in `contraption.rs`), clippy has no warnings.

Seen working:

- A mechanical piston facing up with two poles behind it and a stone on top, driven by a creative motor at 16 RPM:
  turned one way it extends (the piston MOVING, then EXTENDED), the poles and head sliding up through it with the
  stone, drawn moving smoothly, the stone placed two blocks up; turned the other way it retracts, leaving the
  stone (not sticky), the poles back behind it and the piston RETRACTED.
- A sticky piston pulls the stone back down with it.
- A survival player standing on the stone rides it up (from y 102 to 104) instead of falling through.
- Quitting the world in the middle of a retraction saves it (`create_contraptions_*.txt`, offset 1.25) and it
  finishes after loading.
- A horizontal piston pushing a stone toward obsidian stops on the grid with the stone against it.
- Placed by a player looking down, a piston faces up (as `DirectionalAxisKineticBlock` places it).

Seen in the part 2a pass: a slime block on a sticky piston brings the stone beside it, a wall torch on that stone
and a chest beside the slime (its 5 diamonds kept) up and back down; a pig standing on the slime rides up and down;
a piston broken mid-move puts its head, the slime, the stone, the torch and the chest down on the grid (the chest
still holding its diamonds).

Not checked yet: honey, ladders, blocks dropped where they can't land, items carried, long pole chains (the 64-pole
cap), crafting the pistons and poles, the piston and pole models in every facing and axis, a LAN guest seeing it
move.

## Combined pass over milestone 4 (macOS, offscreen)

After part 3 was merged, one world, the same build. 123 unit tests pass, clippy has no warnings.

Fixed here: **the arm's selection line named belt parts by their hidden block** ("Extract from
create:hz_belt_middle"): it now shows the block's translated name, a belt's, funnel's or tunnel's parts by their
base block ("Extract from Belt").

Seen working:

- Fluids: the pump wrenched round empties the tank out of its open end, leaving a water source; a lava source
  pumped into an empty tank, the lava drawn in its window untinted; a water bucket into a basin (drawn in it) and
  wheat flour thrown in make dough under a mixer at 32 RPM, the water used up. (At 16 RPM nothing happens, as in
  Create: the mixer needs `SpeedLevel.MEDIUM`, 30 RPM.)
- Deployer facing west with an iron pickaxe: the wrench on its front switches it to punching, it breaks the stone in
  front, keeps the cobblestone in its overflow and wears the pickaxe; switched back to using with oak planks, it
  waits while its overflow holds anything (as Create), an andesite funnel on its side takes the cobblestone out,
  then it places the planks in front. (The funnel also takes its held item once that's left, as Create's
  `DeployerItemHandler` gives it.)
- Arm: a belt segment as input and a basin as output: the arm takes iron off the moving belt into the basin.
- Vault: breaking the block holding the iron drops it; the two blocks left stay one vault (a hopper over one, a
  funnel on the other take it out).
- Brass funnel as a belt funnel over the belt, filtered to gold: gold pulled off the belt into the chest behind it,
  iron goes on; its filter saved over a reload.
- Breaking a belt segment under a brass tunnel: the whole belt goes (its connector drops), the tunnel breaks off and
  drops, the funnel on it stays.
- Regressions: a press over a depot still presses iron into sheets; funnels on chests and deployers, belts and
  depots as above.

Still not checked from milestone 4: lava and water meeting in a pipe, pipes over 16 blocks, branches, a pump into a
pump, cauldrons and hives; the arm with millstones, chutes, deployers and funnels as points, round robin between
outputs, saving mid-move; the deployer on a belt, an axe on copper, comparing its pole closely; a 2×2×2 vault and the
wrench on a vault; andesite tunnels' windows and shapes changing as neighbours come and go; connected brass tunnels
over parallel belts and an empty hand taking their stacks; a second player for all of it.

## Milestone 4, part 3: checked on the Mac (macOS, offscreen)

Built with `tools/horizon.py build` against the pinned Horizon and horizon-mixins, the content mod from Create
6.0.10's jar, run offscreen. 123 unit tests pass, clippy has no warnings.

Fixed here: **nothing could be placed on a belt** (a tunnel, or a funnel on its top): the belt's right click took
every click so that its grindstone template's menu wouldn't open, where Create's `BeltBlock.useItemOn` passes other
items on to their own use. A held block item is now placed against the belt through Horizon's placement rules
(`place_against` in `logistics.rs`, the face's centre standing for the hit position, which the hook doesn't give).

Seen working:

- Crafting recipes for the brass funnel and both tunnels are in the content mod (carried from the jar).
- Brass funnel on a chest's side holding gold and iron: its iron filter (set by a right click on its front with an
  ingot) makes it take out the whole iron stack and leave the gold; the filter item is drawn on the funnel.
- Andesite tunnel placed on a belt by hand: it turns into a cross between two depots, and a stack of 8 passing
  under it leaves one ingot on each depot and goes on.
- Brass tunnel in its place: Split spreads 9 ingots 3 / 3 / 3 over the two depots and the belt on; the wrench
  switches it to Round robin ("Brass Tunnel: Round Robin" on the action bar), then two stacks of 4 go whole, one
  to each depot; the mode is saved (`create_logistics_*.txt`).

Still not checked (from the list below): brass funnels collecting and as belt funnels, the filter over a reload and
on vertical funnels, tunnel shapes as neighbours change, windows, a tunnel breaking off, connected brass tunnels
over parallel belts, an empty hand taking a group's stacks, brass funnels filtering a tunnel's sides, and the
regressions listed.

## Milestone 4, part 3: brass funnel and belt tunnels (cloud)

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). **Not built with `tools/horizon.py build` and not run in the game.**

| Check | How | Result |
|---|---|---|
| Rule modules (all of `src/lib.rs`, now with `tunnel.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` | 123 tests pass (8 new), no warnings |
| The bundle applies and the game compiles | `python3.12 tools/horizon.py check --horizon ../horizon --no-audio` (Linux: the container has no ALSA) | Pass, no warnings |
| `prepare_content.py`'s funnel and tunnel blocks | `fit_logistics` run on a scratch tree with Create 6.0.10's generated `andesite_tunnel`, `brass_tunnel`, `brass_funnel` and `andesite_funnel` blockstates (from the source tag, not committed) | Each tunnel shape gets a block with `axis=x/y/z` variants (y as x), its loot table in 26.3's format and its base's `mineable` tag; the brass funnel gets its extracting and collecting blocks as the andesite one |
| `tools/horizon.py build`, the content mod from the jar, the game | Not run | — |

What the unit tests cover: tunnel block names and states; `getTunnelState` (T left/right and cross from belts across
it and side inputs, a belt along it not counting, windows between tunnels, a closed window kept); sides and flaps of
straight, T and cross tunnels, flaps closed by a neighbouring tunnel; `canSurvive`; connected brass tunnels
(`getAdjacent`, `isConnected`, `hasDistributionBehaviour`), `gatherValidOutputs` (own sides first, the belt's end,
not into a solid block, nothing on a stopped belt); `distribute` in Split (nine items over three belts, three each;
overflow from a nearly full target to the others) and Round robin (whole stacks in turn, waiting on a full target);
the tick's timing (targets, then two ticks for a new item); a stack entering an andesite cross giving one item to
each side and going on, a single item passing, a brass tunnel taking a stack and holding the next item back at
0.99; the brass funnel's blocks, filter (`test`, set, cleared), amount (a full stack, one for an andesite
extractor) and filter slot positions.

Not checked yet (part 3):

- Crafting the brass funnel and both tunnels (their recipes should be carried from the jar as the andesite funnel's).
- Brass funnel: placing (extracting, sneaking for collecting), breaking with a pickaxe and dropping itself; the
  filter set and cleared by right clicks on its front (any side of an upright one), and not by clicks on other
  faces; the filter item drawn in its slot (position, tilt, size, flat items against block items) on horizontal
  and vertical funnels; extracting a whole stack of the filtered item from a chest with mixed items; collecting
  only the filtered item; as a belt funnel, pulling only the filtered item off the belt (others pass, or wait in
  front of a funnel facing them) and pushing whole stacks onto it; an arm depositing into it; the filter kept over
  a save and reload.
- Andesite tunnel: placing on a belt (the straight block turning to the belt's axis), its shape changing as belts,
  depots and funnels come and go beside it, the window between two tunnels and the wrench closing it; splitting a
  stack onto belts and depots at its sides; breaking off when the belt below goes; drops; models and collision.
- Brass tunnel: a T over a belt with a side belt splitting stacks; three tunnels side by side over parallel belts
  sharing stacks evenly; Round robin after the wrench (the action bar line, the whole group switching); a tunnel
  placed beside a group; an empty hand taking the stacks; a broken tunnel dropping its stack; items thrown off a
  belt's end through it; stacks and modes kept over a save and reload; brass funnels on the side belts filtering
  what a side takes.
- Regressions: andesite funnels and chutes (they now gather an item from every slot, up to a stack, as Create's
  `ItemHelper.extract`), belts without tunnels, arms depositing into andesite funnels.

## Milestone 4, part 2 and fluids: checked on the Mac (macOS, offscreen)

Both cloud PRs merged together, then built with `tools/horizon.py build` (Python 3.12) against the pinned Horizon
(`aa27eca`) and horizon-mixins (`3f99a5c`), the content mod made from Create 6.0.10's jar, and run offscreen in a
creative world. 115 unit tests pass, clippy has no warnings.

Fixed here:

- **The arm and the pump had the same wire code** (12) in `kinetics.rs`'s `create:kinetics` sync once both PRs were
  in: the arm is 14 now.
- **The content mod didn't build**: horizon-port-pack leaves out `mechanical_arm` (no template has its properties)
  and `fit` read its declaration as if it were carried; containers it leaves out are declared from nothing now.
- **Nothing drew a tank's or basin's fluid**: the fluid's sprite was looked up as `minecraft:block/water_still`, and the
  atlas keeps vanilla's sprites without the namespace (`block/water_still`); and the tint was packed as 0xRRGGBB
  where the vertex takes RGBA8 (red in the low byte), which turned water orange.
- **A right click on a deployer gave its item straight back**, and every Create block's click could run twice:
  Horizon's client doesn't run the mod, so for a block whose template has no use it sends the off hand's use too,
  where Create's client answers SUCCESS and sends none. `glue::right_click` drops the off hand's click on a block the
  main hand's click just took (same tick, same block).
- **The deployer's andesite casing never came** (nor any recipe naming `c:stripped_logs`, `c:cobblestones`,
  `c:stripped_woods`...): NeoForge's convention tags for vanilla items are added to the content mod
  (`CONVENTION_TAGS`); tags only other mods fill stay empty, as with NeoForge.
- **Vaults placed by a command never joined**: the vault's block entity starts `Uninitialized` now
  (`ItemVaultBlock.onPlace` runs however the block is placed), not only when a player places it.

Seen working after the fixes:

- Fluids: a mechanical pump driven by a cogwheel beside it (−16 RPM) between an open pipe end in a water source and a
  fluid tank: the source drained and its 1000 mB went into the tank within a few seconds, saved and loaded with the
  world (`create_fluids_*.txt`). Buckets: a water bucket fills the tank and an empty one takes it back in creative
  (the bucket is kept, as Create's `FluidHelper` does); since the multiblock pass, survival buckets on tanks pass
  as in Create. The water is
  drawn in the tank's window, tinted.
- Arm: the arm item selects a depot ("Extract from depot") and another ("Deposit to depot", after a second click),
  the placed arm takes both points, turns with a cogwheel beside it and carries iron ingots from one depot to the
  other; its base, arms, claw and cog are drawn.
- Deployer: placed facing down by a command over a depot, its shaft turned by a motor along Z; a right click on its
  front gives it andesite alloy; it waits while the depot is empty (as Create's `shouldActivate` with a transported
  item handler below) and applies the alloy to stripped oak logs on the depot, making andesite casing (8 alloy left
  6 after two logs); pole and hand drawn, its state saved in `create_deployers_*.txt`.
- Vault: three blocks in a row join; a hopper fills the first block, an andesite funnel on the last one takes the
  iron out of the whole vault.
- Crafting: horizon-port-pack drops the crafting recipes of the blocks it leaves out; `prepare_content.py --jar` now
  carries the ones for blocks it declares (andesite funnel, fluid pipe, fluid tank, item vault, mechanical arm,
  deployer), and NeoForge's vanilla convention tags those and the carried recipes name (`c:barrels/wooden`,
  `c:dyes/*`, `c:chests/wooden`...). In survival, alloy over dried kelp in the inventory grid gave 2 andesite
  funnels, and copper sheet, barrel, copper sheet at a crafting table a fluid tank.

## Milestone 4, fluids (cloud)

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). Then checked on the Mac: see above.

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`, `belt.rs`, `chute.rs`, `funnel.rs`, `fluids.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` | 89 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check --no-audio` (Linux; the container has no ALSA, so the audio crates are left out) | Pass, no warnings |
| `prepare_content.py`'s straight-pipe blockstate | Run by hand on Create 6.0.10's generated `fluid_pipe.json` (from the source tag, not committed) | Each axis gets the two body parts of a straight pipe |
| Content mod from Create 6.0.10's jar, `build`, the game | Not done (no jar or Minecraft resources in the cloud) | — |

What the unit tests cover: NeoForge's `FluidTank` fill and drain; straight pipes and pumps open along their axis;
open ends draining a source and pouring by the bucket (`removeFluidFromSpace`, `provideFluidToSpace`, the
obsidian/cobblestone/stone collisions, water evaporating in the nether, Create's honey from a full hive); a pump
between a water source and a tank putting pressure on both sides (`distributePressureTo`, values per face), then
moving water into the tank at half its speed a tick (16 mB a tick at 32 RPM) once the flows are complete; a stopped
pump moving nothing; a pump turned round emptying the tank into the world, as a source block once the open end holds
a bucket; removing a pipe wiping the pressure; saving pipes and tanks; the tank's fluid box (`FluidTankRenderer`);
the pump as a cog about its facing (no shaft, 4 su, wrench turns it round, saved); the basin's tanks (inputs kept
apart, outputs drained first, `CombinedTankWrapper`'s loss of a second fluid), its fluid boxes (`BasinRenderer`)
and recipes with fluid ingredients (one tank per ingredient, `SizedFluidIngredient`), and which mixing recipes are
supported (dough and cardboard pulp yes; tea, chocolate and honey no: no milk or Create fluids here).

Not checked yet (fluids):

- The pump's cog turning, seen; goggles on the pump; the pipe and pump breaking with a pickaxe and dropping
  themselves; placing pipes by hand along the clicked axis.
- The pump wrenched round emptying the tank out of the open end; lava; water and lava meeting in a pipe; pouring onto
  lava; the nether.
- An infinite pool feeding the pump for long (the test pool had air under it, so its source didn't come back: a
  test rig problem, not the mod's), the tank filling to 8000 mB.
- Pipes longer than 16 blocks, two branches, a pump into a pump, breaking a pipe mid-flow, a block in front of an open
  end.
- Buckets on a basin and its fluids drawn; a mixer making dough from flour and piped water; cardboard pulp.
- Cauldrons and beehives drained by an open end; a LAN guest seeing tanks and basins fill.

## Milestone 4, arm and deployer (cloud)

Ported in a cloud session on Linux (x86_64), Rust 1.99.1, Python 3.12, against the pinned Horizon (`aa27eca`) and
horizon-mixins (`3f99a5c`). Then checked on the Mac: see above.

| Check | How | Result |
|---|---|---|
| Rule modules (all of `src/lib.rs`, now with `vault.rs`, `arm.rs`, `deployer.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` | 101 tests pass (26 new), no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check --no-audio` (Linux: the container has no ALSA, so the server build without audio; nothing of the mod is behind that feature) | Pass, no warnings |
| `prepare_content.py`'s new `fit_arms` | Run on a scratch tree with Create 6.0.10's generated `deployer.json` blockstate | Both deployer blocks get their `facing` variants, the extra one a 26.3-format loot table dropping the deployer |
| `tools/horizon.py build`, the content mod from the jar, the game | Not run | — |

What the unit tests cover: vaults joining along their axis, the 3×width length cap, crossing axes, a 2×2 vault and its
slot order, splitting when a block goes and forming again (`ConnectivityHandler`); the arm's full cycle (search,
move, collect, search outputs, deposit), its move time (`speed / 1024` a tick), not taking what no output can take,
a stopped or redstone-locked arm, funnels being deposit-only, the 5-block range, `ArmAngleTarget`'s base angle and
reach, backing up to rest before reaching out, points saved relative to the arm; the deployer's cycle timings
(`getTimerSpeed`, 1000 out, activation, 1000 back, 500), the overflow, the hand offset, its rotation axis,
placement (`axis_along_first` by neighbouring shafts), the wrench, `BeltDeployerCallbacks`, reading deploying
recipes; the arm and deployer as kinetic blocks (a cog drives the arm, the deployer's shaft follows
`axis_along_first`, saved and synced kinds).

Not checked yet (arm, deployer, vault):

- Crafting the fluid pipe, item vault, mechanical arm and deployer at a table (their recipes are carried from the
  jar now as the funnel's and tank's, which were crafted: see above).
- Vault: placing by hand (its axis from the player or a lone vault beside it), a 2×2×2 vault, chutes and arms on a
  vault, breaking a block of a multiblock, the wrench from above, right clicks not opening the barrel's menu.
- Arm: belts, basins, millstones, chutes, deployers and funnels as points; round robin; out-of-range points on
  placing; an empty hand taking its item; the held item drawn in the claw while moving; saving mid-move; a second
  player.
- Deployer: placing by hand (facing, sneaking, `axis_along_first` by a shaft); the wrench switching punch/use;
  using (placing a held block), punching (breaking the block in front); deploying on a belt; an axe on copper; the
  pole and hand compared closely against Create's (the pole stands out of the back while retracted, as the
  partial's 9 px behind the block suggest, but its movement wasn't compared frame by frame); dropping its items when
  broken.
- Regressions: belts, chutes and funnels into chests and depots, presses over belts and depots (they now look for
  vaults and deployers first). The combined pass below was made before this merge.

## Combined pass over earlier milestones (macOS, offscreen)

Checked in one world after milestone 4 part 1 was merged, with the same build:

- Depot: a hopper above fills it and a hopper below empties it (3 iron ingots through to a chest).
- Millstone: a hopper above feeds it wheat, an andesite funnel on its side takes the flour out.
- Press over items on the ground: 2 iron ingots pressed into 2 iron sheets. **Fixed here**: the scan tested the
  item's feet against the deflated box under the press, so an item lying on the floor was never found; it now
  tests the item's bounding box, as `getEntitiesOfClass` does.
- Fan: pushes items on the ground along its current; through a lit soul campfire it turns sand into soul sand.
- Wrench: a creative motor and a press turn a quarter about the clicked top face; a fan clicked on a face along its
  axis stays (as Create's `getRotatedBlockState`); sneak + wrench in survival picks up a depot into the inventory and
  drops its items.
- Water wheel in a water trough: −8 RPM from the flow, driving two shafts beyond it.
- Belt: the far pulley drives a shaft beyond it (16 RPM); an andesite funnel on a chest over the belt pushes the
  chest's items onto it, one at a time.
- Andesite funnels: one on a chest's or millstone's side extracts one item and waits until it is gone (as Create's
  funnel with no filter); a collecting funnel on a chest takes items dropped onto it.
- Chute: a hopper above it feeds a chest below through it.

## Milestone 4, part 1

Ported in a cloud session on Linux (x86_64), then built, fixed and checked on macOS (Apple M5), Rust 1.99.1,
Python 3.12, against the pinned Horizon (`aa27eca`) and horizon-mixins (`3f99a5c`).

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`, `belt.rs`, `chute.rs`, `funnel.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -D warnings`, `cargo test --locked` | 75 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check` (Linux) and `build` (macOS) | Pass, no warnings |
| Content mod from Create 6.0.10's jar | `tools/prepare_content.py --jar` | Pass: belt part blocks, chute, both funnel blocks, loot tables |
| In the game (offscreen, `horizon-ctl`, macOS) | Rigs built with commands, screenshots and frame differences, `/data get block` | See below |

Seen in the game:

- Belt connector: shaft, then a shaft 6 blocks along: a belt of start, 5 middles and end; used up in survival. A
  sloped belt (one up per block) is made and drawn diagonally.
- Items dropped on it lie flat, move apart at the belt's speed, stop at a wall or are thrown off the end; the
  belt texture scrolls (23 % of its pixels change between frames, the floor's none). Belt into depot and into a
  chute over a chest hand items over. Belt items are back after closing and reopening the world. An empty hand
  takes a segment's items.
- A press over the belt presses a stack of 3 iron ingots into 3 iron sheets, one per cycle. A fan through a lit
  campfire smokes 2 raw beef waiting at the belt's end.
- Breaking a segment in survival with a pickaxe: the connector drops, the chain becomes air with shafts at both
  ends, its items drop.
- Chute: items fall through three chutes and wait in the lowest over stone (as in Create).
- Andesite funnel on a chest extracts its items.

Fixed on the Mac: breaking a belt left the start pulley as air (the chain was removed over several ticks and the
controller broke itself first; now the chain is walked at once, as `onRemove` does); belt parts dropped nothing
(loot tables in 1.21.1's format, and the parts missing from `mineable/*`); a press on a belt let the rest of a
stack pass after the first item (its results were not locked, as `BeltPressingCallbacks` does).

Not checked yet: a belt funnel pulling items off the belt into its inventory, a second player.

## Milestone 3, part 2

Ported in a cloud session on Linux (x86_64), then checked on macOS (Apple M5), Rust 1.99.1, Python 3.12, against
the same pinned Horizon (`aa27eca`) and horizon-mixins (`3f99a5c`).

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`, `fan.rs`, `obj.rs`) | `cargo fmt --all --check`, `cargo clippy --all-targets --locked -D warnings`, `cargo test --locked` | 54 tests pass, no warnings (Linux and macOS) |
| The bundle applies and the game compiles | `tools/horizon.py check` and `build` (Python 3.12) | Pass, no warnings (Linux and macOS) |
| Content mod from Create 6.0.10's jar | `tools/prepare_content.py --jar` | Pass; the water wheel's OBJ, MTL and `wheel.json` are in it |
| In the game (offscreen, `horizon-ctl`, macOS) | Rigs built with commands, screenshots, `/data get block` | See below |

Seen in the game:

- Encased fan behind a creative motor at 16 RPM: the current finds water and lava in front of it. 2 raw iron on a
  depot behind lava became 2 iron ingots; 4 gravel on a depot behind water became 1 flint (the recipe's 25 % flint
  and 12.5 % nugget rolls), after about 7.5 s.
- Mechanical mixer driven by a cogwheel beside it (it has no shaft, as in Create) at 32 RPM, over a basin:
  2 andesite and 2 iron nuggets dropped into the basin became 2 andesite alloy in its output slots. Pole, head,
  casing and the basin are drawn.
- Creative motor's value box: "Rotation Speed / Scroll to change" over the crosshair; the mouse wheel steps
  16 → 20 → 24, and ⟲ renders in Horizon's font.
- Wrench: a click on a shaft's south face turns its axis x → y; sneak + click in survival picks it up into the
  inventory.
- Water wheel: its frame and OBJ wheel drawn in spruce.

Not checked yet: the fan pulling (reversed), slabs in the current; the OBJ wheel seen turning; a LAN guest changing
a value box.

## Milestones 1 to 3, part 1

What has been checked, on macOS (Apple M5), Rust 1.99.1, against Horizon Nightly 2026-10-31 (`aa27eca`) and
horizon-mixins `3f99a5c` (the revisions in `compatibility.json`).

| Check | How | Result |
|---|---|---|
| Rule modules (`kinetics.rs`, `processing.rs`) | `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test` | 30 tests pass, no warnings |
| The bundle applies and the game compiles | `tools/horizon.py check` / `build` (Python 3.12) | Pass, no warnings |
| Content mod from Create 6.0.10's jar | `tools/prepare_content.py`, then Horizon's `cargo horizon-mod build` | Pass |
| In the game (offscreen, `horizon-ctl`) | Rigs built with commands, screenshots and frame differences, `/data get block` | See below |

Seen in the game:

- Shafts, cogwheels (reversing), large cogwheels, gearbox (sides by source), creative motor, hand crank turn and
  are drawn turning; overstress and breaking on conflict per the unit tests.
- Engineer's goggles: press 128 su at 16 RPM of a creative motor's 262144 su; water wheel 256 su at 8 RPM.
- Depot: an item placed from the top; 3 iron ingots pressed into 3 iron sheets under a press, one per cycle, the
  head going down.
- Millstone: wheat thrown on it milled into wheat flour.
- Water wheel: water flowing over it gives a flow score of 2 and 8 RPM.

Not checked yet: a LAN or dedicated server with a second player, building through the Horizon launcher, and anything against Create running
on NeoForge (no oracle yet: values are compared by hand with Create's code).

## Belt tunnels placed on belts (macOS, offscreen)

- An andesite tunnel is refused on stone and placed on a flat belt, where it turns to the belt's axis (`axis=x` on an
  eastward belt) at its next tick. A tunnel placed with the template's default axis (`y`) read as X before, so it
  stayed unturned; fixed.
- Seen in a screenshot: the tunnel over the belt. The belt's casing under it and the brass tunnel's flaps swinging
  when it distributes (the funnel's flaps swing, same code) are not checked in the game yet.

## More Create sounds (macOS, offscreen)

- A hand crank turned by a right click: `create:cranking` and its compounded part are heard (at most one per ten ticks, as
  `tickAudio` plays one every ten ticks while the crank is in use). Not checked in the game yet: the mixer's mix sound
  at its twentieth tick and the funnel's flap sound (both compile and share the sound path).
- Super glue held: the first corner and the second corner each played `create:slime_added` and the item frame's click
  (heard in `status`); taking glue off plays the low slime sound (not checked in the game).

## Chassis range display (macOS, offscreen)

- A linear chassis with a sticky top and a stone column above it, the wrench held and pointed at the chassis: the
  column is outlined in white thin lines (screenshot). The group display with Ctrl, the fade-out after the wrench is put
  away and the checkered face texture are not checked in the game (the texture is not drawn).

## Super glue's selection preview (macOS, offscreen)

- Super glue held, the first corner chosen on a stone block and the crosshair on another: the box between them is
  outlined in green (screenshot). The red/yellow failing colour, the cluster outline and the thin outlines of glue
  already placed are not checked in the game.
- Super glue in the off hand, a stone placed against a stone with the main hand: the two are glued (the glue's thin
  outline around both shows with the glue in the main hand afterwards, screenshot). The glue's use is counted out of
  creative only (not checked).

## Mechanical saw facing up (macOS, offscreen)

- A saw facing up driven by a creative motor: an andesite alloy dropped on it goes into its inventory, is cut after
  about 10 s at the motor's 16 RPM (`ProcessingTime` counting down from 200 by one a tick in the block data), the stone
  sound plays and six shafts are thrown off the east side (`/data get entity`). A belt carrying two andesite alloys into
  the saw's side: it took one at a time, cut it into six shafts, and while the belt ahead moved against it the
  shafts waited in its slots (as Create's `canInsertFromSide` makes them); with the saw turned the other way they
  were thrown off the open side. Not checked: a depot or a belt moving away taking the pieces, wood and stone sounds by
  name.

## Deployer goggle lines (macOS, offscreen)

- Engineer's goggles on, looking at a deployer on a creative motor: "Deployer Information" and "Mode: Use" above the
  kinetic stats and the network line (screenshot). The held item's line (with an item in its hand) is not checked
  in the game.

## Brass tunnel distributing (macOS, offscreen)

- A brass tunnel over a belt with a depot on each side: it turns into the cross shape, an iron ingot stack riding the
  belt through it is split, one ingot into each depot, and the east, west, north and south flaps swing (`Flap<side>`
  counters in its block data). The goggle tooltip (`Currently distributing:`) is compiled only: the stack is held
  for too short a time to look at in an offscreen run.
- The saw's filter: an iron ingot set on its top (a click with it in hand, `Filter` in the block data); an andesite
  alloy dropped on it then came out uncut (no recipe with an iron ingot output). The filter item drawn on the saw's
  top was not looked at in a screenshot.
- The deployer's goggle line with an item: `Item: Iron Ingot x4` shows when the deployer holds four iron ingots.
- The deployer's filter: an iron ingot set by a click on its top; andesite alloy put in its hand by a click on its
  front was sent away (no item line on the goggles), iron ingots stay (the line shows). The overflow's contents and the
  save and load of the filter were not looked at.

## Belt funnel shapes (macOS, offscreen)

- Funnels set above a belt turn to the belt funnel states at the next tick: across the belt an extracting one is
  `shape=pushing` and a collecting one `shape=pulling`, along it `shape=retracted`, off a belt `shape=wall`
  (`/data`-less: `horizon-ctl block`). Screenshot: the belt funnels' own models stand on the belt. The flap models'
  offsets and the shapes' collision were not checked.
- The wrench on a belt funnel along a belt: `shape=retracted` becomes `shape=extended` and back (`horizon-ctl block`).
  The extended model, its flap offset and a tunnel's window beside it were not looked at.

## Speedometer and stressometer (macOS, offscreen)

- A speedometer and a stressometer on a creative motor's shaft line: their blocks place and join the network, the
  heads with their dials are drawn on the two open sides, and the goggles on the speedometer read `Gauge Information:`,
  `Rotation Speed`, a bar and `16 RPM` (screenshot). The stressometer's goggles read `Network Stress`, `Low` and
  `Remaining Capacity 262144su` (screenshot); the speedometer's dial needle shows turned. The overstressed state was not looked at; the tooltips' exact bar and colours are from memory of `SpeedLevel` and
  `StressImpact` (their classes are not in the sources kept here).

## Clutch (macOS, offscreen)

- A creative motor, a clutch and a speedometer in a line: 16 RPM on the goggles; a redstone block beside the clutch
  (`powered=true`) drops the speedometer to 0 RPM (screenshots). The gearshift's reversal is covered by a unit test
  only; the shaft halves' turning was not looked at.
- The deployer's filter item (an iron ingot) shows lying on its top face (screenshot); the other filter side and the
  orientation on sides other than the top were not compared with Create's.

