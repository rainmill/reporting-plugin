# Changelog

Notable changes to `rainmill-reporting`. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) as Cargo reads it: before 1.0 a minor
bump is breaking and a patch is additive.

## [Unreleased]

### Added

- First cut. Report dashboards for Laterite: a data source contract with periods, comparison and caching, indicators, line, bar, horizontal bar, doughnut, pie, percentage and table widgets, YAML dashboards on a 20-column grid, and a data source over any table.
- Each dashboard is a Laterite surface: a caller reads its period and each widget's settings as controls, and reads an indicator's, chart's, table's or breakdown's figures for the period and settings it gives, with nothing saved.
- A dashboard is a Laterite board: it opens at once and each widget loads its own data, and Edit dashboard moves, resizes, adds, removes and configures the report widgets, saved per person or for everyone.
