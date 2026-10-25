// Generated from config/deployment-identities.json. Do not edit.
import Foundation

enum DitchIdentity {
#if DITCH_STAGING
  static let environment = "staging"
  static let app_name = "Ditch Staging"
  static let app_id = "ai.theditch.app.staging"
  static let runtime_id = "ai.theditch.runtime.staging"
  static let obsolete_runtime_id = "ai.theditch.status.staging"
  static let support_directory = "The Ditch Staging"
  static let helper_name = "The Ditch Staging Runtime"
  static let keychain_service = "ai.theditch.installation-identity.staging.v1"
  static let ssh_keychain_service = "dev.theditch.ssh-password.staging.v1"
  static let remote_directory = ".ditch-staging"
  static let remote_service = "the-ditch-staging.service"
  static let remote_label = "dev.theditch.runtime.remote.staging"
  static let remote_keychain_service = "dev.theditch.remote-control.staging.v1"
  static let url_scheme = "theditch-staging"
  static let url_name = "ai.theditch.agent.staging"
#else
  static let environment = "production"
  static let app_name = "Ditch"
  static let app_id = "ai.theditch.app"
  static let runtime_id = "ai.theditch.runtime"
  static let obsolete_runtime_id = "ai.theditch.status"
  static let support_directory = "The Ditch"
  static let helper_name = "The Ditch Runtime"
  static let keychain_service = "ai.theditch.installation-identity.v1"
  static let ssh_keychain_service = "dev.theditch.ssh-password.v1"
  static let remote_directory = ".ditch"
  static let remote_service = "the-ditch.service"
  static let remote_label = "dev.theditch.runtime.remote"
  static let remote_keychain_service = "dev.theditch.remote-control.v1"
  static let url_scheme = "theditch"
  static let url_name = "ai.theditch.agent"
#endif
  static let supportRelativePath = "Library/Application Support/" + support_directory
}
