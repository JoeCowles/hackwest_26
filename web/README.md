# Orchard Cluster Console

Static implementation of the *Orchard Cluster Console* design (Industry design system: Barlow Condensed over Barlow, steel-blue accent, square hairline frames with "+" registration marks).

No build step. Serve the folder and open `index.html`:

```sh
cd web && python3 -m http.server 8080
# → http://localhost:8080
```

Modules are loaded as ES modules, so the page must be served over HTTP (not `file://`).

## Layout

| File | Role |
| --- | --- |
| `index.html` | Shell, fonts, mounts the app |
| `css/console.css` | Tokens and component classes |
| `js/app.js` | State, hash routing (`#fs`, `#node/pippin`, …), timers, modal |
| `js/views.js` | The six views plus sidebar, top bar and page modal |
| `js/model.js` | View models: per-host derived values, live jitter, KPI tiles |
| `js/stage.js` | three.js rack elevation (drag to orbit, hover for vitals, click to open) |
| `js/data.js` | Cluster fixture data |
| `js/charts.js` | Deterministic series + SVG polyline helpers |

## Runtime dependencies (CDN)

- Preact + htm: `unpkg.com/htm@3.1.1/preact/standalone.module.js`
- three.js `0.149.0` from jsDelivr, loaded lazily with a 3 s budget. If it fails the overview shows a fallback panel and every other view is unaffected.
- Google Fonts: Barlow, Barlow Condensed
