---
id: ids-not-names
type: principle
name: IDs, not names
---
# IDs, not names

Every [[concept:reference]] in a corpus is a stable typed ID, never a bare display name.
This single choice is the reason a knowledge base authored as flat prose eventually breaks:
names change, get typo'd, get translated, or turn out to refer to two different things,
while an ID doesn't move once it's chosen.

The payoff is that a node's [[concept:display-name]] is resolved fresh every time it's
rendered, from the target's current `name:` field. Rename an [[concept:entity]] once —
edit a single `name:` value — and every reference to it anywhere updates instantly, because
none of those references ever stored the old name to begin with; they stored the address.
The same discipline extends to [[concept:supersession]]: a tombstoned ID keeps resolving
forever precisely because nothing downstream ever depended on the string, only on the
`type:id` pair.
