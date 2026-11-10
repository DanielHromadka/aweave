# aWeave website

Static page, no build step: `index.html`, `style.css`, `site.js`, `install.sh` and `img/`. Meant for Cloudflare Pages
(project `aweave`, https://aweave.pages.dev) connected to this repository, publishing this folder on every push to
`main` (build command: none, output directory: `website`).

```bash
python3 -m http.server 8765 -d website    # http://localhost:8765/
```

## Downloads

`site.js` lists the archives `release.yml` publishes under stable names (`aweave-aarch64-apple-darwin.tar.gz`, …),
the visitor's own build first, and fills in sizes and the version from the GitHub API. Without the API (offline,
rate limit) the links fall back to `/releases/latest/download/<name>`.

`install.sh` is served from here so `curl -fsSL https://aweave.pages.dev/install.sh | sh` works; it downloads from the
same release and checks the `.sha256` file next to the archive.

## Design notes

Same system as afinder.pages.dev: Geist and Geist Mono, light or dark with the system, buttons and chips at 10px
radius, panels and code at 16px. One accent, the violet of the icon. No screenshots: the terminal blocks show real
output of the CLI. Motion is limited to the hero entrance, scroll reveals and the dashed flow in the queue diagram,
all off under `prefers-reduced-motion`. Fonts (Fontsource) and icons (Phosphor) load from jsDelivr and unpkg at
pinned versions.
