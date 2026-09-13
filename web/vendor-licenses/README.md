# Embedded frontend dependencies

These files were downloaded on 2026-09-12. Runtime assets are embedded in the
Rust binary and served by Orchard; browsers make no requests to these sources.

| Asset | Pinned upstream source | License |
| --- | --- | --- |
| `js/vendor-preact-htm.js` | https://unpkg.com/htm@3.1.1/preact/standalone.module.js | htm Apache 2.0; bundled Preact MIT (`htm.txt`, `preact.txt`) |
| `js/vendor-three.js` | https://cdn.jsdelivr.net/npm/three@0.149.0/build/three.min.js | MIT (`three.txt`) |
| `css/fonts.css` | Google Fonts Barlow / Barlow Condensed v13 WOFF2 responses, embedded as data URLs | SIL Open Font License 1.1 (`barlow.txt`, `barlow-condensed.txt`) |

The htm asset is the original standalone bundle, including the Preact version
bundled by its publisher. It was not rebuilt against a newly resolved dependency.
The JavaScript files retain upstream bytes. The font stylesheet replaces Google
Fonts URLs with their downloaded WOFF2 payloads; existing Latin, extended Latin,
and Vietnamese subsets and weight selections are retained.

SHA-256:

```text
72284e8e9079c87817145df1110f74e8a2aa040b2fc384922e18dfcb46fc1fd7  js/vendor-preact-htm.js
8a5f7249903b54d30f79f708699d2fed2d6a1d0741a4cd41377d1f01bb5a2271  js/vendor-three.js
37aecfe7d0aae53fb31a1313bac9edb08561dd4e520d22d70a29d728b5e7764f  css/fonts.css
```
