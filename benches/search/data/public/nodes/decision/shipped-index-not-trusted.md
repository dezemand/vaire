---
id: shipped-index-not-trusted
type: decision
name: "The shipped index is a claim, never truth"
---
# The shipped index is a claim, never truth

A pulled [[concept:artifact]] carries a prebuilt index, and materializing it into the
[[concept:store]] throws that index away entirely and rebuilds it from the shipped
Markdown using the consumer's *own* copy of Vairë. Adopting a publisher's build directly
would make every consumer's answers depend on a stranger's toolchain, and a corpus whose
index quietly disagreed with its own text would have no way of ever being caught.

Rebuilding costs roughly a second per package, which is cheap enough that this decision
removes an entire class of trust question for free. What *does* travel with the artifact is
provenance — which commit the files came from, and how they were indexed — because that's a
fact about the release itself, not a claim about the graph woven from it. This is the same
instinct as [[principle:files-are-authoritative]] applied one layer further out: even
someone else's index is still just a cache, never something to believe on its own.
