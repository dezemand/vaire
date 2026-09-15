---
id: record
type: concept
name: Record
---
# Record

A record is a [[concept:node]] describing something that happened or was produced —
meeting notes, a decision, a status update — rather than a thing with standing identity.
Records reference [[concept:entity|entities]] by ID and are commonly nested under a
container via [[concept:scoped-id|scoping]], but scoping and record-ness are independent:
a record just happens to be the conventional case that wants a container.

The defining rule is **immutability**: a record is additive-only. To change what it says,
write a new record — never edit an existing one's prose. The single sanctioned exception is
reference resolution, which only adds an ID target and keeps the original wording as
display text (see [[principle:additive-authoring]]). This is what makes autonomous
authoring safe: a wrong record is just a wrong record, locally contained and superseded by
whatever gets written next, never a lie a later editor has to go back and unwrite.
