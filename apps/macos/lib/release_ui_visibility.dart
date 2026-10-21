/// Presentation only: hide unfinished feature controls for this release.
/// Keep feature implementations, registrations, callbacks and saved state intact.
/// Set to true when these surfaces are ready to return to the UI.
const showUnfinishedFeatureControls = false;

/// GitHub/Board rollout is limited to the staging Relay and staging builds.
const showGitHubBoardControls =
    String.fromEnvironment(
          'DITCH_DEPLOYMENT_ENVIRONMENT',
          defaultValue: 'production',
        ) ==
        'staging' &&
    String.fromEnvironment('DITCH_RELAY_ORIGIN') ==
        'https://ditch-remote-relay-staging.matin-1a7.workers.dev';
