# Pure core with effects

blorp's state is owned by one task and changed only through `update(&mut App, Msg) -> Vec<Effect>`, a synchronous function with no I/O. Network calls and quitting are returned as effects that a thin executor carries out, feeding results back as messages. We chose this over async methods on the app so that behaviour such as reconcile, retries, and the disconnected state is tested by feeding messages and asserting on state and effects, with no runtime and no server. The cost is an explicit message and effect vocabulary that every new feature extends.
