# Keyboard shortcuts

Generated from the binding table in `crates/flock-ui/src/keymap.rs`; run `mise run gen:keys` after changing it. The same table binds the keys and draws the Keyboard shortcuts section of Settings (⌘,).

## General

| Keys | Action |
|---|---|
| ⌘P | Quick open: resources, clusters and pages |
| ⌘⇧P | Run a command |
| ⌘K | Command palette |
| ⌘⇧C | Switch cluster |
| ⌥⌘R | Refresh this view |
| ⌘0 | All clusters in this tab |
| ⌘, | Settings |
| ⌘N | New window |
| ⌘⇧W | Close the window |
| ⌘Q | Quit Flock |

## Tabs

| Keys | Action |
|---|---|
| ⌘T | New tab on Flock home |
| ⌘W | Close the tab; closing a pane's last tab closes the pane |
| ⌘\ | Split right: open the tab in a second pane |
| ⌥⌘1 / ⌥⌘2 | Focus pane 1 or 2 |
| ⌃Tab / ⌥⌘→ | Next tab |
| ⌃⇧Tab / ⌥⌘← | Previous tab |
| ⌘1 / ⌘2 / ⌘3 / ⌘4 / ⌘5 / ⌘6 / ⌘7 / ⌘8 / ⌘9 | Select tab 1 to 9 |
| ⌘-click / middle click | Open a page, row or result in a new tab |
| middle click | Close the tab under the pointer |
| drag | Reorder tabs, or drop one on the other pane's strip to move it |
| right click | Tab menu: move to the other pane or a split, close |

## View

| Keys | Action |
|---|---|
| ⌘B | Toggle the sidebar |
| ⌘J | Toggle the bottom dock |
| ⌘⇧E | Focus the sidebar |
| ⌘E | Focus the page's list |
| Tab / ⇧Tab | Next or previous focus area |
| / | Focus the page filter |
| ⌘F | Focus the page filter |

## Selecting text

| Keys | Action |
|---|---|
| drag | Select text |
| double click / triple click | Select a word, a line |
| ⇧-click | Extend the selection |
| ⌘C | Copy the selected text |
| ⌘A | Select all of the clicked text block |

## Navigate

| Keys | Action |
|---|---|
| ⌘[ | Back |
| ⌘] | Forward |
| ⌃- | Back, also in the YAML editor where ⌘[ outdents |
| ⌃⇧- | Forward, also in the YAML editor where ⌘] indents |

## Lists and tables

| Keys | Action |
|---|---|
| ↑ / ↓ | Move the selection |
| Home / End | First or last row |
| PgUp / PgDn | Move a page |
| letters | Jump to the row whose name starts with them |
| ↵ | Open the selected row |
| Esc | Close the inspector (not on Pods, where a pod is always selected) |
| ⌘. | Connect, disconnect or remove the highlighted cluster in the switcher |

## Selected resource

| Keys | Action |
|---|---|
| ⌘L | Follow logs |
| ⌘⇧D | Compare across clusters |
| ⌘⇧G | Show the Flux owner |

## Command palette

| Keys | Action |
|---|---|
| ⌘↵ | Open the result in a new tab |
| ⌥↵ | Open the result in a new window |
| ⌘⇧↵ | Open the result in a split |

## Workload page

| Keys | Action |
|---|---|
| ⌘. | Actions |
| ⌘L | Logs |
| ⌃` | Terminal |
| ⌘Y | YAML |

## Terminal

| Keys | Action |
|---|---|
| Tab | Complete in the shell |
| ⇧Tab | Send back-tab to the shell |
| ⌘C / ⌘V | Copy the transcript, paste into the shell |

## Flux object page

| Keys | Action |
|---|---|
| ⌘R | Reconcile |
| ⌘⇧R | Reconcile with source |
| ⌘D | Diff against Git |
| ⌘Y | YAML |

## Editing YAML

| Keys | Action |
|---|---|
| ⌘E | Edit the YAML on screen |
| ⌘↵ | Review changes, then apply |
