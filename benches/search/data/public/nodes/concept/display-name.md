---
id: display-name
type: concept
name: Display name
---
# Display name

The text a rendered reference shows. `[[type:id]]` with no pipe takes it from the target's
`name:` field; `[[type:id|Custom text]]` overrides it. The fallback chain when `name:` is
absent: the sole `# H1` heading in the prose, then the filename without its extension — a
missing or ambiguous H1 (not exactly one) skips straight to the filename.

Because the display text is resolved at render time rather than stored, renaming a node —
editing one `name:` field — updates every reference to it everywhere, instantly, without
touching a single referencing file. This is the entire payoff of [[principle:ids-not-names]]:
the address (`type:id`) never moves, only what it prints as.
