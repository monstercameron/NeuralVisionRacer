# Third-party reference sources

Pinned subsets of three PlayCanvas projects, kept so the Rust port can be checked against the originals. These are
reference copies, not buildable copies of the projects.

| Folder | Project | Licence |
| --- | --- | --- |
| `playcanvas-engine/` | playcanvas/engine | MIT |
| `splat-transform/` | splat-transform | MIT |
| `supersplat-viewer/` | supersplat-viewer | MIT |

`upstream-lock.json` records each file's path, the commit it came from and its Git blob hash. Check the files with
`python3 scripts/fetch_upstream.py` (run from `splat-racer/`); it downloads any missing file and refuses to overwrite a
modified one.
