# Levels

Scene folders the game streams from. The folder is in the repository's working tree, but git ignores its contents
(only this README is committed): the level data is large and each scene has its own licence.

| Folder | Scene | Creator | Licence | Playable |
| --- | --- | --- | --- | --- |
| `nmbu-park-03` | NMBU park_03 | ramzihassan | Not specified on the public page: all rights reserved | yes (default level) |
| `trompetstraat-delft` | Trompetstraat, Delft | roelof | CC BY-SA 4.0 | yes |
| `basilica-assumption` | Basilica of the Assumption | tanha | CC BY-NC-ND 4.0 (non-commercial, no derivatives) | yes |
| `saburi-alley-yazd` | Saburi alley, Yazd | parsamahdavi48 | CC BY 4.0 | yes |
| `madras-square` | Madras Square | dronesatwork | Not specified on the public page: all rights reserved | no (no collision voxels yet) |

Each folder has a `README.txt` (what it is, its licence link) and a `source.json` (the scene URL and checksums of every
file).

Each level contains `lod-meta.json`, its chunk folders, `scene.voxel.json` and `scene.voxel.bin` (collision), and
`viewer-settings.json` (the spawn hint and camera). The game reads them through `engine.conf` (`[level] path`, which
defaults to `levels/nmbu-park-03`) and the browser page's `?level=` menu.

**Before publishing anything derived from these scenes, check the licence:** basilica forbids commercial use and
derivatives, Delft requires share-alike, and two have no stated licence.
