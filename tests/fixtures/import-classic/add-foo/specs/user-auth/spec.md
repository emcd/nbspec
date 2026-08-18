# user-auth

## ADDED Requirements

### Requirement: User authentication
The system SHALL authenticate users before granting access.

#### Scenario: Valid login
- **WHEN** a user submits correct credentials
- **THEN** a session begins

## MODIFIED Requirements

### Requirement: Password rules
The system SHALL require passwords of at least twelve characters.

#### Scenario: Short password
- **WHEN** a user submits a password under twelve characters
- **THEN** the system refuses the submission with a diagnostic