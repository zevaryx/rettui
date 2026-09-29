# Fonts

The fonts are [Fira Code](https://github.com/tonsky/FiraCode) as patched by
[Nerd Fonts](https://github.com/ryanoasis/nerd-fonts) (release 3.x,
`FiraCode.tar.xz`), licensed under the SIL Open Font License 1.1: see
[OFL.txt](OFL.txt).

They're split in two, so a page only downloads what it shows:

- `FiraCodeNerdFont-Regular-Text.woff2` and `FiraCodeNerdFont-Bold-Text.woff2`:
  everything but Nerd Font's icons (about 90 KB each).
- `FiraCodeNerdFont-Icons.woff2`: the icons, in the private use areas (about
  1 MB). `style.css` gives it that `unicode-range`, so browsers fetch it only
  for a page that uses one.

Made from the patched fonts' TTF files with fontTools
(`pip install fonttools brotli`):

```python
from fontTools.ttLib import TTFont
from fontTools import subset

def icon(c):
    return 0xE000 <= c <= 0xF8FF or c >= 0xF0000

def make(source, target, pick):
    font = TTFont(source)
    options = subset.Options()
    options.flavor = "woff2"
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=[c for c in font.getBestCmap() if pick(c)])
    subsetter.subset(font)
    font.flavor = "woff2"
    font.save(target)

make("FiraCodeNerdFont-Regular.ttf", "FiraCodeNerdFont-Regular-Text.woff2", lambda c: not icon(c))
make("FiraCodeNerdFont-Bold.ttf", "FiraCodeNerdFont-Bold-Text.woff2", lambda c: not icon(c))
make("FiraCodeNerdFont-Regular.ttf", "FiraCodeNerdFont-Icons.woff2", icon)
```

rettui's web UI serves them itself (`/fonts/…`), so browsers never fetch fonts
from a third party.
