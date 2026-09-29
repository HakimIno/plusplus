# Monochrome provider marks

These are adaptations of the provider SVGs previously bundled in this directory:

- `postgres.svg`: `skill-icons--postgresql-dark.svg`, without the blue fill;
  tighter viewBox and slightly stronger outline for small sizes.
- `mysql.svg`: `skill-icons--mysql-dark.svg`, with a tighter viewBox.
- `mariadb.svg`: `simple-icons--mariadb.svg`.
- `sqlserver.svg`: the symbol from `devicon-plain--microsoftsqlserver-wordmark.svg`,
  without the wordmark.
- `sqlite.svg`: the feather from `skill-icons--sqlite.svg`, without its tile
  background, gradient, and wordmark, cropped to the feather.
- `cassandra.svg`: `simple-icons--apachecassandra.svg`.
- `scylladb.svg`: `simple-icons--scylladb.svg`.

Visible shapes use white so egui can tint them with the theme's text colour.
Transparent backgrounds work on selected rows and on either theme. Each provider
uses one SVG instead of separate light/dark variants. DuckDB uses the shared
Tabler database icon.
