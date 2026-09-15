---
id: gate-the-rare-act
type: principle
name: Gate the rare act
---
# Gate the rare act

The rule that decides what an autonomous agent may do without asking: gate only the rare,
semantic, irreversible acts, and leave the frequent, safe, additive ones completely free.
Instructing an agent to "be careful" doesn't scale — the fix is structural, making the
dangerous choice literally unavailable on the everyday path rather than trusting judgment
every single time.

In practice this draws exactly one line. Writing a [[concept:record]], extending an
entity's prose, adding an alias, leaving a [[concept:loose-end]] — all additive, all
locally contained, all safe to do constantly without review, because a wrong record is
just a wrong record. Minting a brand-new [[concept:entity]], by contrast, is the one act
pulled out of the autonomous path entirely and handed to the deliberate
[[concept:entity-creation-pass]] — because an ID gets referenced everywhere once it exists,
so a duplicate or a mis-merged one poisons every record that ever pointed at it. The same
shape repeats at the package level: a MAJOR version bump is a semantic claim about meaning
that only a maintainer can make (see [[decision:computed-bumps]]), while every MINOR and
PATCH is computed and adopted silently.

What makes this safe to hand to an autonomous writer is that the dangerous decision simply
never reaches them — there's no rule to follow about *not* creating an entity, because
creating one was never an option they had in the first place.
