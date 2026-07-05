# retired-features Specification

## Purpose
Baseline capability whose requirements the fixture change retires and
renames.

## Requirements

### Requirement: Legacy toggle
The system SHALL honor the legacy toggle.

#### Scenario: Toggle honored
- **WHEN** an operator sets the legacy toggle
- **THEN** the system honors it

### Requirement: Feature flag
The system SHALL gate features behind a flag.

#### Scenario: Flag gates a feature
- **WHEN** a feature flag is disabled
- **THEN** the gated feature stays inactive
