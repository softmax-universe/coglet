# App integration

How an app becomes something a cogent can use. The app's developer exposes one
or more **adapters**; we run **coglets** that drive the app through them. Blender, Chrome,
Minecraft, Mindustry and phone apps all integrate the same way. Part of the
[cogent design](README.md).

## Where a runtime lives

The same coglet and the same adapter work in all three placements. The choice
is about latency and isolation, per app.

| Placement | How coglet and adapter meet | When |
|---|---|---|
| Inside the app | In the same process | The app is ours, or wants to embed a runtime. |
| On the user's machine | A local link | The app does not host a runtime itself. Another app on the machine that hosts one, such as the Softmax app, runs the coglets for it. |
| On Softmax infrastructure | Over the network | The app is remote, or the coglet runs on a Softmax-hosted runtime. |

An app may have several adapters (one for its UI, one for a player, one for a
game master), and a runtime may be connected to several adapters at once.

## Adapter

The adapter sits between the runtime and the real world: a microphone, a
speaker, an app such as Blender or Chrome, a job scheduler. It holds no coglet logic. An
adapter is built by the app's developer and exposes only what they choose, in
the way a web API or an MCP server does. It speaks the messages below to one runtime over a private local link. How that
link is carried (a separate process, or the runtime embedded in the app) is a
deployment detail and changes none of the messages.

### `AdapterApi`

What the runtime may ask of an adapter.

```idl
AdapterApi
  observe(Read) -> ReadReply
      Return one bounded snapshot of something in the app. No side effects.
  act(Act) -> Accept
      Admit or reject one action. If accepted, the adapter later sends exactly one Event of kind
      `action.outcome` carrying an Outcome.
```

### `RuntimeApi`

What an adapter sends to its runtime.

```idl
RuntimeApi
  hello(Hello) -> HelloReply
      Open the session and agree a protocol version. Always first.
  capabilities(Capabilities)
      Advertise everything this adapter can do. A full replacement.
  grants(Grants)
      Say which coglet may use which operation. A full replacement.
  event(Event)
      Report something that happened. `source` is this adapter.
  health(Health)
      Heartbeat, sent by both sides at least once a second.
  kill(Kill)
      Announce that the off-switch was already pulled. Not a request.
```

### Session types

```idl
Hello
  adapter         AdapterId
  offer           VersionOffer  `{versions: ["0.1"]}`.
  clock           u64           The adapter's clock now, so deadlines can be mapped onto it.
  correlation_id  Id

HelloReply
  selection       VersionSelection  `{version: "0.1"}`.
  correlation_id  Id

Capabilities
  revision        u64         Starts at 1 and rises with every replacement.
  clock           u64
  entries         [CapEntry]
  correlation_id  Id

CapEntry
  # One group of related operations, for example `speech.control`.
  name       OpName
  events     [OpName]       Event kinds the adapter may send.
  reads      [OpName]
  actions    [OpName]       Operations a coglet may `emit`.
  exclusive  [OpName]       Resources only one coglet may own, for example `audio.speaker`.
  schemas    {OpName: str}  A pinned schema URI for every name above.

Grants
  epoch           u64      Rises with every replacement. Requests carrying an old epoch are
                           denied.
  entries         [Grant]
  correlation_id  Id

Grant
  coglet      CogletId
  operations  [OpName]  Operations this coglet may use.
  resources   [OpName]  Exclusive resources this coglet owns.
  expires     u64       Adapter-clock time at which the grant lapses.

Health
  state           "ready" | "degraded" | "stopped"
  clock           u64
  pending         u64                             Admitted actions that have no outcome yet.
  correlation_id  Id

Kill
  reason          str
  correlation_id  Id
```

### Request types

```idl
Act
  grant_epoch  u64     The grants epoch the runtime believes is current.
  action       Action

Read
  coglet          CogletId
  capability      OpName
  args            ProfileMap
  grant_epoch     u64
  deadline        u64
  correlation_id  Id

ReadReply
  correlation_id  Id
  ok              bool
  value           ProfileMap?
  snapshot        u64?         A revision number for the snapshot. Present when ok.
  error           Error?
```

### Adapter duties

- Validate identity, epoch, operation, resources, arguments and deadline before accepting.
- Send exactly one outcome for each accepted action, and none for a rejected one.
- Reject any deadline more than 60 seconds ahead.
- Treat a repeated `(coglet, id)` as a retry: return the stored answer, never run it twice.
- Stop all output and revoke all grants on kill or disconnect.

### Several adapters on one runtime

A runtime may hold sessions with several adapters at once, for example one for
audio and one for a display.

- Each adapter has its own session, its own grants and its own off-switch.
- An operation name belongs to exactly one adapter, so the runtime always knows
  where an action goes. Two adapters advertising the same name is an error.
- A coglet's manifest can draw on operations from any of them.

The spec describes a single session and does not yet say how several coexist,
so this still needs to be written down.

## App coglets

App coglets are the coglets that drive apps: for example `BlenderCoglet`,
`MinecraftCoglet`, `BrowserCoglet` for Chrome, `MindustryCoglet`, and coglets
for phone apps.

Coglets and apps are many-to-many:

- **Several coglets can drive one app.** Minecraft might have one coglet
  playing a character and another building in the world, each through a
  different adapter, or both through the same one.
- **One coglet can drive several apps.** A coglet that helps with research
  might use Chrome and a notes app together.

Whichever way they are split, app coglets have the same shape. Each keeps a
skill library and what it has learned in its own memory, takes a `UserIntent`,
and acts through the adapters it has been granted.

### `AppCoglet`

Written for the simplest case, one coglet driving one app, with `<app>`
standing for the app's own name, for example
`blender.act`.

```idl
AppCoglet
  subscribes  <app>.state, <app>.event, <app>.result
  needs       <app>.act, <app>.query, llm.complete, intent.report
  owns        (whatever its adapter makes exclusive)

  provides  intent.request      UserIntent      addressed
            Look in the skill library. If a skill fits, run it through the adapter. If none
            does, report `needs_help`.
  on        <app>.state         AppState
  on        <app>.event         AppEvent
  on        <app>.result        AppResult
  calls     intent.report       IntentReport    addressed
```

### App adapter

A developer can expose whatever they like. As a starting point, this is a
simple shape with five operations that fits most apps. What goes inside them is
up to the app's developer.

```idl
-- events, adapter to coglet

AppState
  revision  u64
  diff      ProfileMap          Changes since the previous revision, limited to what the coglet
                                may see.

AppEvent
  type      str                 Something that happened in the app.
  data      ProfileMap

AppResult
  # The answer to one query.
  query     Id
  ok        bool
  value     ProfileMap?
  cursor    str?                Pass it back for the next page.
  error     Error?

-- actions, coglet to adapter

AppAct
  # `<app>.act`. Changes something in the app. The adapter decides whether it is allowed.
  verb      str                 One of the verbs this adapter offers.
  params    ProfileMap

AppQuery
  # `<app>.query`. The answer arrives as an `<app>.result` event with the same id.
  query     Id
  what      str                 One of the things this adapter can be asked.
  params    ProfileMap
  cursor    str?
  limit     int 1..64
```

A developer who does not want to design verbs can offer one: a verb that runs a
script in the app's own language. That is their choice to make, and the coglet
then carries its skills as scripts.

| App | Adapter is | Verbs it might offer | A coglet that might drive it |
|---|---|---|---|
| Blender | an add-on | `run_script`, or `add_object`, `transform`, `render` | `BlenderCoglet` |
| Chrome | an extension | `navigate`, `click`, `type`, `run_script` | `BrowserCoglet` |
| Minecraft | a client mod | `move`, `place`, `break`, `craft` | `MinecraftCoglet` |
| Mindustry | a game mod | `move`, `mine`, `build`, `research` | `MindustryCoglet` |
| A phone app | built into the app | whatever the developer exposes | one or more |

The verbs in this table are examples, not a catalog. An app can also offer more
than one adapter, for example one for a player and one for a game master.
