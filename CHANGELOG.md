# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- System clipboard on Wayland, through `wl_data_device`.
- Following dark mode and accent-color changes while the app runs, on Windows,
  macOS and Linux: `Event::ThemeChanged`, `Window::set_dark` and
  `Theme::follows_system`.

## [0.0.2](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.1...stipple-v0.0.2) - 2026-11-07

### Fixed

- fix clippy chunks_exact_to_as_chunks lint (new in stable clippy)

### Other

- merge the toolkit into a single stipple crate
