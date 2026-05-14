# Spark — Game Design

## Pitch

You are an energy planner, not a hands-on builder. Across a small region with growing with main city and side cities, you decide *what* power infrastructure to build and *where*. Workers come from the main city you supply; they travel, construct, maintain, and operate the facilities you plan and build. Cities grow when their power demand is met and decline when it isn't. Over time you progress from a single water wheel powering a hamlet to an atomic grid feeding a metropolis and side cities.

Think SimCity's growth pressure × Settlers × Factorio × Anno's supply chains × Dwarf Fortress's "designate, don't control" interaction model, focused entirely on the energy stack.

## Design pillars

1. **Indirect control.** The player never moves a worker, never clicks "build now". The player designates intent; the simulation decides timing, allocation, and outcomes.
2. **Energy progression as the spine.** The whole tech tree is energy: combustion → hydro → wind/solar → fossil → nuclear → (stretch) fusion. Every new tier reshapes how you plan.
3. **Cities as both demand and labour.** A city is a meter and a workforce simultaneously. Power it well and it grows, giving you more workers and more money. Power it badly and it shrinks, starving your construction queue.
4. **Geography matters.** Rivers, wind exposure, sun, terrain elevation, and proximity placement non-trivial. There are no "best" plants — only locally optimal ones.
5. **Legible simulation.** Every system the player encounters can be inspected. No black boxes. Overlay views for power flow, worker assignment, demand heatmap, pollution.
6. You sell energy to cities, not to a faceless grid. The player is a regional planner, not a utility company. You get money back to spend on expansion and upgrades, but you don't micromanage pricing or compete with other suppliers.

## Core game loop

Per minute of play:

1. **Inspect** — review power balance, city growth, current builds, worker availability.
2. **Plan** — designate a new build (plant, transmission line, housing).
3. **Adjust** — re-prioritise queued work; assign workers to mines/plants; toggle plants on/off; sell surplus power.
4. **Observe** — watch workers travel, construct, plants ramp up, demand shift, city tick over.
5. **Research** (occasional) — unlock the next tier of generation, city auto upgrade.

Per session:

1. Start with a hamlet and an empty grid.
2. Build first generator (water wheel or wood burner).
3. Connect first city.
4. Watch city grow → more demand → more workers available.
5. Outgrow primitive gen → tech up.
6. Grow main city sell energy to side cities, balance the grid.
7. Reach atomic age.

## Indirect control — the mechanic in detail

The player's tool set is **plans, designations, and policies** — never direct commands.

### Plans

A *plan* is a placed blueprint: "build a coal plant here", "lay a transmission line from A to B"

A plan has state:

- `Planned` — drawn but not yet started
- `Surveying` — workers measuring (short delay)
- `UnderConstruction` — workers actively building, progress bar
- `Operational` — building works
- `Maintenance` — temporary, workers servicing
- `Decommissioning` — being torn down

The player can place, cancel, or re-prioritise plans. The player cannot fast-forward construction except via the global speed control.

### Worker autonomy

Workers come from cities. Each city tick:
- Produces N idle workers (proportional to population, employment rate, education).
- Idle workers pick the highest-priority job they're qualified for within travel range.
- Workers commute, work, return home, sleep.

Worker categories (start simple, expand later):
- **Labourer** — construction, mining, basic maintenance
- **Engineer** — operates and maintains complex plants (gas, nuclear)
- **Specialist** — research, advanced repair (later tier)

The player can shift priorities ("more engineers, fewer labourers" via city policy) but cannot directly assign individuals.

## Power network

A graph of producers, transmission lines, and consumers.

### Producers
Each plant has:
- Nominal output (MW)
- Fuel/resource consumption rate (if any)
- Build cost, build time, worker requirements (during build and during operation)
- Maintenance interval and cost
- Terrain requirements (river for hydro, wind score for wind, etc.)
- Money

### Transmission
- Lines have capacity (MW) and length-dependent loss (% per km)
- Substations split/merge lines and step voltage
- A line is a chain of segment entities or a graph edge — TBD by ECS design (lean toward graph edge in `PowerNetwork` resource)

### Consumers
- Cities, factories, all consume power
- Demand varies by time of day (day/night cycle) and season
- Brownouts when supply < demand → city growth penalty, industry slowdown

### Solving the grid
Each simulation tick, distribute available power to consumers by priority:
1. Critical infrastructure (mines, plant auxiliaries, hospitals)
2. Residential
3. Industry / surplus export

If demand exceeds supply, lower-priority consumers brown out first. This is a simple priority-based allocation, not a true AC load flow — we're not building a power systems simulator.

## Cities

A city is an entity (or small set of entities) with:
- Population (citizens)
- Workforce (employable adults)
- Demand (kW, varies by tier and population)
- Tier (hamlet → village → town → city → metropolis)
- Happiness / growth modifier
- Worker output rate (per game tick)
- Housing capacity

### Growth model
Each tick:
- If demand met AND happiness > threshold → population grows
- If demand unmet OR happiness < threshold → population stagnates or shrinks
- At population thresholds, city tiers up → unlocks new buildings, more demand, more workers

### Demand sources
- Residential (constant baseline scaling with pop)
- Lighting (cyclic, peaks at night)
- Industry (varies with active workplaces)
- Climate control (peaks in season extremes — stretch goal)

## Tech progression — energy tiers

| Tier | Tech | Output | Requirements | Notes |
|------|------|--------|--------------|-------|
| 0 | Water wheel | Tiny | River tile | Always available, cheap |
| 0 | Wood burner | Tiny | Forest nearby | Cheap, consumes wood |
| 1 | Windmill | Small | Open terrain, wind score | Variable output |
| 1 | Coal burner | Small | Coal mine nearby | Reliable, polluting |
| 2 | Hydro dam | Medium | Large river / elevation | Expensive build, long life |
| 2 | Coal plant | Medium | Coal supply chain | Scales bigger than burner |
| 3 | Gas turbine | Medium | Gas deposit / pipeline | Fast ramp, dispatchable |
| 3 | Wind farm | Medium | Open terrain cluster | Renewable |
| 4 | Solar array | Medium-large | Sun score, large flat land | Daytime only |
| 4 | Oil plant | Large | Oil deposit | High output, polluting |
| 5 | Nuclear (fission) | Huge | Uranium supply, engineers | Long build, high stakes |
| 5+ | Fusion | Vast | Stretch goal | Endgame |

Each unlock costs research points generated by educated workers in cities. Research is a long-cycle resource — you commit to it via policy.

### Pollution & happiness
Coal, oil, and (mildly) gas reduce happiness in cities within range. Renewables and nuclear (when safe) do not. Pollution is a soft constraint, not a hard fail.

## Economy (v1 — minimal)

- **Capital** — currency, spent on construction and upkeep
- **Income** — selling power to cities (price per kWh × consumption)
- **Costs** — build cost, fuel cost, worker wages, maintenance
- **Loans** (stretch) — to bootstrap large builds

Bankruptcy = game over (or pause + bailout in easy modes).

## Time

Real-time with speed control: pause / 1× / 2× / 5×. One in-game day ≈ 2 real minutes at 1×. Day/night affects solar output and lighting demand; seasons (stretch) affect heating/cooling demand.

Simulation tick = 60 Hz internally (fine-grained), with major game events (worker decisions, city growth, plant output recalc) on a slower beat (e.g. once per in-game minute).

## Map & world

- 2D top-down, tile-based grid (decision: simplest for learning + fits the aesthetic).
- Tile types: grass, forest, mountain, river, lake, coast, desert (each affects what can be built).
- Resource deposits scattered on the map: coal, gas, oil, uranium.
- Map size: start with 64×64 for v1 prototypes, scale to 256×256 or larger as the engine handles it.
- One map, one play session — sandbox mode for v1. Scenarios and multiple maps later.

## What's in v1 (MVP scope)

The minimum-viable-Spark needed to call it a "game":

- One map, one starting city
- Three plant types: water wheel, coal burner, windmill
- Workers (one category: labourers)
- Construction, operation, maintenance loop
- Power network with transmission lines
- City demand + growth + decline
- Game over: bankruptcy or city collapse
- No save/load yet (stretch)
- No tech tree UI yet (manual unlocks for testing)

Anything beyond is post-v1.

## Out of scope (v1)

- Multiplayer (forever, likely)
- 3D rendering
- Mod support
- Multiple maps / campaign mode
- Disasters (storms, meltdowns) — added in v2
- Pollution as a hard mechanic
- AC load flow simulation
- Realistic geopolitics

## Open questions

These need a decision before milestone work starts touching them, but not on day 1:

- **Camera**: pure top-down 2D, or 2.5D isometric? Top-down is simpler. Isometric reads more "city builder" but doubles the rendering work.
- **Tile size**: how big is a tile relative to a city or a plant? A coal plant probably spans multiple tiles; a water wheel is one tile. Need a consistent unit.
- **Day/night length**: 2 real min/day is a default; real value tunes pacing.
- **Worker travel time**: do workers walk (slow, visible), teleport (fast, no anim), or use vehicles?
- **Power line aesthetic**: visible pylons or abstract glow?
- **Failure UX**: how does the game communicate that a city is browning out? Overlay, sound, alert panel?

## Inspirations (for reference)

- *Factorio* — tech progression, supply chains, build queue mentality (but we are far less hands-on)
- *Dwarf Fortress* / *RimWorld* — designate-don't-control
- *Anno 1404 / 1800* — supply chains feeding cities, demand-driven growth
- *Frostpunk* — policy as gameplay
- *Power Grid* (board game) — the economics of producing and selling electricity
- *SimCity 2000* — power grid, demand zones, city tiers
- *Captain of Industry* — recent indirect-control reference, comparable scope
- *Settlers* — worker autonomy, building placement
