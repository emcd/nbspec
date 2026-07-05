## ADDED Requirements

### Requirement: Basic capability
The system SHALL provide the basic capability to operators.

#### Scenario: Capability engaged
- **WHEN** an operator engages the capability
- **THEN** the system responds with a confirmation

### Requirement: Layered capability
The system SHALL layer the basic capability behind an interface.

#### Scenario: Interface consulted
- **WHEN** a caller consults the interface
- **THEN** the layered capability answers

#### Scenario: Interface bypass refused
- **WHEN** a caller bypasses the interface
- **THEN** the system refuses the request
