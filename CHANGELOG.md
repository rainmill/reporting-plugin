# Changelog

Notable changes to `rainmill-reporting`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) as Cargo reads it: before 1.0 a minor
bump is breaking and a patch is additive.

## [Unreleased]

### Added

- First cut. Report dashboards for Laterite: a data source contract with periods, comparison and caching, indicators, line, bar, horizontal bar, doughnut, pie, percentage and table widgets, YAML dashboards on a 20-column grid, and a data source over any table.
- A dashboard opens at once and each widget loads its own data, so a slow source delays its own widgets alone; widgets pack under the one above in their columns.
- Edit dashboard: move, resize and remove widgets, saved as your own layout or as the one everyone starts from.
