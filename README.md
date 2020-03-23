# Cord

Cord is a data streaming platform for composing, aggregating and distributing arbitrary
streams. It uses a publish-subscribe model that allows multiple publishers to share their
streams via a broker. Subscribers can then compose custom sinks using a regex-like
pattern to access realtime data based on their individual requirements.

Cord has a programmable interface in the form of the [Client library](client/), so it can
be adapted to various use cases. As the library matures, this library will form the basis
of a suite of adaptors for common technologies.

## Usage

The easiest way to get up and running is by using the Client CLI. In most production
scenarios, you would implement the Client library independently.

#### 1. Start a new broker instance

By default, the `--bind-address` is _127.0.0.1_, and the `--port` is _7101_.

    $ ./server &

#### 2. Subscribe to an arbitrary namespace

In this example, we will subscribe to the `/names` namespace.

    $ ./cord sub /names

Whenever a new event is published under this namespace, it will be printed to stdout.

#### 3. Publish to this namespace

First, declare that you will provide the `/names` namespace.

    $ ./cord pub /names

Next, enter key-value pairs into the console, using the format `NAMESPACE=VALUE`. For
example:

> /names=pete  
> /names/fictional=Homer Simpson

## Modules

Cord comprises a number of modules:

-   **[Client](client/)** - provides a library and CLI for interacting with Cord brokers
-   **[Message](message/)** - provides the core `Message` enumerator and codec for
    sending `Message`s between brokers and clients
-   **[Pattern Matcher](pattern-matcher/)** - provides the algorithm for comparing
    namespace patterns
-   **[Pub/Sub](pubsub/)** - provides a `Publisher` and `Subscriber` to represent the
    stream and sink components of a client connection within a broker
-   **[Server](server/)** - the server binary that aggregates and distributes arbitrary
    streams

## Etymology

Cord derives its name from the _spinal cord_. The long term goal of this project is to
aggregate data from every part of your environment, so that the decisions you make within
that environment are better informed. This is akin to the central nervous system, which
uses the spinal cord to aggregate electrical feedback from nerve endings.
