# Cord

**This is a placeholder crate for the Cord Project. Please do not use this crate!**

Cord is a data streaming platform for composing, aggregating and distributing arbitrary
streams. For more information, please stop by the project website
([cord-proj.org](http://cord-proj.org)) and check us out on GitHub
([cord-proj](https://github.com/cord-proj)).

## Crates

Cord comprises three crates:

1.  **[Cord Broker](https://crates.io/crates/cord-broker)** - the server binary that
    aggregates and distributes arbitrary streams
2.  **[Cord Client](https://crates.io/crates/cord-client)** - provides a library and CLI
    for interacting with Cord brokers
3.  **[Cord Message](https://crates.io/crates/cord-message)** - an internal crate that
    defines the message envelope and codec for transmitting messages over the wire
