# Contributing Guide

Thank you for your interest in contributing to Terminal! This document provides guidelines and information for contributors.

## Getting Started

### Prerequisites

- Rust 1.70 or later
- Git
- A GPU with Vulkan/Metal/DX12 support (for testing)

### Setting Up the Development Environment

1. Clone the repository:
   ```bash
   git clone https://github.com/user/terminal
   cd terminal
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Run the tests:
   ```bash
   cargo test
   ```

4. Run the terminal:
   ```bash
   cargo run
   ```

## Development Workflow

### Branching Strategy

- `main` - Stable release branch
- `develop` - Development integration branch
- `feature/*` - Feature branches
- `fix/*` - Bug fix branches

### Making Changes

1. Create a feature branch from `develop`:
   ```bash
   git checkout develop
   git checkout -b feature/my-feature
   ```

2. Make your changes
3. Write tests for new functionality
4. Ensure all tests pass:
   ```bash
   cargo test
   ```

5. Commit your changes with a descriptive message
6. Push to your fork and create a pull request

### Commit Messages

Use conventional commit messages:

- `feat: Add new feature`
- `fix: Fix bug in X`
- `docs: Update documentation`
- `test: Add tests for Y`
- `refactor: Refactor Z`
- `perf: Improve performance of W`

### Code Style

- Follow Rust standard style guidelines
- Use `cargo fmt` to format code
- Use `cargo clippy` to check for lints
- Add documentation comments for public APIs
- Write tests for new functionality

## Architecture

The terminal is organized into four layers:

### 1. PTY Layer (`src/pty/mod.rs`)

- Manages pseudoterminal creation
- Handles shell process spawning
- Provides read/write interfaces

### 2. Parser Layer (`src/parser/mod.rs`)

- Implements VT100/VT220 state machine
- Based on Paul Williams' parser design
- Processes byte sequences into actions

### 3. Grid Layer (`src/grid/mod.rs`)

- 2D array of cells
- Manages cursor position
- Handles SGR attributes
- Implements scrollback buffer

### 4. Renderer Layer (`src/render/`)

- GPU-accelerated text rendering
- Font atlas management
- wgpu pipeline setup

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture
```

### Writing Tests

- Add unit tests for new functions
- Test edge cases and error conditions
- Use descriptive test names
- Follow the existing test patterns

### Test Coverage

We aim for high test coverage. New features should include:

- Unit tests for core logic
- Integration tests for component interactions
- Edge case testing

## Documentation

### Code Documentation

- Add doc comments for all public items
- Include examples where appropriate
- Keep documentation up to date

### User Documentation

- Update README.md for new features
- Add configuration options to docs/CONFIGURATION.md
- Update key bindings in docs/KEYBINDINGS.md

## Reporting Issues

### Bug Reports

When reporting bugs, please include:

- Operating system and version
- Rust version
- Steps to reproduce
- Expected behavior
- Actual behavior
- Screenshots if applicable

### Feature Requests

When requesting features, please include:

- Clear description of the feature
- Use cases
- Potential implementation ideas

## Code of Conduct

- Be respectful and inclusive
- Focus on constructive feedback
- Help newcomers learn
- Celebrate contributions of all sizes

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## Questions?

If you have questions about contributing, feel free to:

- Open an issue
- Start a discussion
- Reach out to maintainers

Thank you for contributing!
