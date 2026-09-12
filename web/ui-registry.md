# Orchard live-console patterns

## Connection and server-backed panels

Preserve the established Barlow body and Barlow Condensed headings, squared
panels, paper-gray background, steel-blue accent (#5980a6), ink (#1d1f20),
and thin rgba(29,31,32,.18) dividers. New styles live in css/live.css.

| Element | Pattern |
| --- | --- |
| Connection panel | Existing panel/panel-pad; subtle blue diagonal wash |
| Inputs | Square, thin border, monospace credential text, visible blue focus |
| Status strip | Left accent border; muted blue background; muted red on error |
| Primary action | Existing btn/btn-primary; never a simulated operation |
| Measurements | Condensed headings; explicit Unknown for missing values |
| Tables | Thin row dividers; uppercase small headings; horizontal mobile scroll |
| Spacing | 20px panel gaps; 12px control gaps; existing panel padding |
| Links | Blue text, underline on hover, keyboard focus outline |

Unknown is neutral gray, not a success color. Stale/disconnected values must not
appear current. Historical/session charts show gaps instead of artificial zeros.
Unavailable features have explanatory empty states, not fixture activity.
