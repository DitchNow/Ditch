import Foundation
import PeekabooAutomationKit
import PeekabooBridge

/// The embedded service assembly. Runtime authorization is enforced before this
/// host is invoked; Peekaboo does not own a model or a separate agent loop.
@MainActor
public final class AutomationHost {
    let services: PeekabooEmbeddedBridgeServices

    public init(coordinationDirectory: URL) {
        self.services = PeekabooEmbeddedBridgeServices(
            desktopMutationWatermarkStore: DesktopMutationWatermarkStore(directoryURL: coordinationDirectory),
            snapshotOptions: .init(snapshotValidityWindow: 30, maxSnapshots: 8,
                                   deleteArtifactsOnCleanup: true, copyArtifactsOnStore: false)
        )
    }

    public func permissions() -> AutomationPermissionState {
        AutomationPermissions.status()
    }
}
