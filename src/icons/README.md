# Icons

`mdi.txt` names the [Material Design Icons](https://pictogrammers.com/library/mdi/)
(version 7.4.47, Apache License 2.0), one per line with its code point:
the names Sideband, Columba and MeshChat give a person's icon (LXMF's icon
appearance field). Only icons in the bundled Nerd Font
(`src/web/assets/fonts/FiraCodeNerdFont-Icons.woff2`, which puts them at the
same code points) are listed, so the web UI can show each one.

Made from `@mdi/font`'s `scss/_variables.scss` with fontTools
(`pip install fonttools brotli`):

```python
import re
from fontTools.ttLib import TTFont

cmap = TTFont("src/web/assets/fonts/FiraCodeNerdFont-Icons.woff2").getBestCmap()
icons = re.findall(r'"([a-z0-9-]+)":\s*(F[0-9A-F]{4})', open("_variables.scss").read())
with open("src/icons/mdi.txt", "w") as out:
    out.writelines(f"{name} {code}\n" for name, code in sorted(icons) if int(code, 16) in cmap)
```
