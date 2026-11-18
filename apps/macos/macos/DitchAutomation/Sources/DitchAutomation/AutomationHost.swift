import Foundation
import PeekabooAutomationKit
import PeekabooBridge

/// The embedded service assembly. Runtime authorization is enforced before this
/// host is invoked; Peekaboo does not own a model or a separate agent loop.
@MainActor
public final class AutomationHost {
    let visualizer = AutomationVisualizer()
    let services: PeekabooEmbeddedBridgeServices
    let feedback: DitchAutomationFeedbackClient
    let foregroundInput: UIAutomationService

    public init(coordinationDirectory: URL) {
        self.feedback = DitchAutomationFeedbackClient(visualizer: self.visualizer)
        self.services = PeekabooEmbeddedBridgeServices(
            desktopMutationWatermarkStore: DesktopMutationWatermarkStore(directoryURL: coordinationDirectory),
            snapshotOptions: .init(snapshotValidityWindow: 30, maxSnapshots: 8,
                                   deleteArtifactsOnCleanup: true, copyArtifactsOnStore: false)
        )
        self.foregroundInput = UIAutomationService(snapshotManager: self.services.snapshots,
            inputPolicy: .init(defaultStrategy: .synthOnly), feedbackClient: self.feedback)
    }

    public func permissions() -> AutomationPermissionState {
        AutomationPermissions.status()
    }
}
