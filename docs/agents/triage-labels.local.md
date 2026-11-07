# Triage labels

The skills speak in terms of five canonical triage roles. This file maps each role to the label string on `alacritree/alacritree`.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill names a role, such as "apply the AFK-ready triage label", use the label string from this table.

The tracker is Arnaud's repository, and not every label in the table exists on it. List the labels with `gh label list -R alacritree/alacritree` before you apply one. Ask Lev before you create a label there.
