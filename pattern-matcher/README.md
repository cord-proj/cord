# Pattern Matcher

The pattern matcher provides a regex-like syntax for matching strings. It can also match
other patterns, allowing users to test whether one pattern encapsulates another. This is
useful for finding publishers that provide data matching a subscriber's namespace
pattern.

## FAQ

### Why not use an off-the-shelf regex library?

Regex libraries will always provide more functionality for matching strings than this
library can. However what they cannot do is compare two regex patterns and determine
whether one encapsulates another, which is crucial to how publishers and subscribers
interact. Without this, the distributed pub/sub system cannot function.
