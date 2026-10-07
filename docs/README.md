# The documentation site

[zevaryx.github.io/rettui](https://zevaryx.github.io/rettui/) is built with
[Zensical](https://zensical.org/) from the pages in [wiki/](../wiki), which
stay the one copy: the GitHub wiki is published from them too. Write and
change pages there.

```sh
pip install -r docs/requirements.txt
python3 docs/build.py           # build the site into docs/_build/site
python3 docs/build.py serve     # build it and serve it at http://localhost:8000, reloading on changes to docs/_build
python3 docs/build.py pages     # only write the pages and config
```

`build.py` writes the pages to `docs/_build/pages` as the site wants them:

- Links between pages (`[Running](Running#commands)`) point at their files,
  and `Home` becomes the front page.
- Each page gets its name from the sidebar as its title.
- GitHub's `> [!WARNING]` notes become admonitions.
- The navigation is [wiki/_Sidebar.md](../wiki/_Sidebar.md), with
  [zensical.toml](zensical.toml)'s theme and settings.

It stops, saying why, on a link to a page or heading that doesn't exist, and
on a page left out of the sidebar. Zensical then builds in strict mode.

The [Docs workflow](../.github/workflows/docs.yml) builds the site for pull
requests that change it, and publishes it to GitHub Pages from `main`.
