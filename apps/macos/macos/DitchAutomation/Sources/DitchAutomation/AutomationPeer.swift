import Darwin
import Foundation
import Security

/// Validate the live process, not a caller-supplied executable name or UID.
@_cdecl("ditch_automation_verify_peer")
public func automationVerifyPeer(_ pid: Int32, _ role: UInt32,
                                 _ bytes: UnsafePointer<UInt8>?, _ length: Int) -> Int32 {
    guard pid > 0, let bytes, (1...8192).contains(length) else { return 1 }
    let expected = String(decoding: UnsafeBufferPointer(start: bytes, count: length), as: UTF8.self)
    var pathBuffer = [CChar](repeating: 0, count: 4096)
    guard proc_pidpath(pid, &pathBuffer, UInt32(pathBuffer.count)) > 0 else { return 1 }
    let path = URL(fileURLWithPath: String(cString: pathBuffer)).resolvingSymlinksInPath().path
    let expectedPath = URL(fileURLWithPath: expected).resolvingSymlinksInPath().path
    guard (role == 1 && path == expectedPath)
        || (role == 2 && path.hasPrefix(expectedPath + "/Contents/MacOS/")) else { return 1 }
    var peer: SecCode?
    let attributes = [kSecGuestAttributePid as String: pid] as CFDictionary
    guard SecCodeCopyGuestWithAttributes(nil, attributes, [], &peer) == errSecSuccess, let peer,
          SecCodeCheckValidity(peer, [], nil) == errSecSuccess else { return 1 }
    var peerInfo: CFDictionary?
    var own: SecCode?
    var peerStatic: SecStaticCode?
    var ownStatic: SecStaticCode?
    var ownInfo: CFDictionary?
    let informationFlags = SecCSFlags(rawValue: kSecCSSigningInformation)
    guard SecCodeCopyStaticCode(peer, [], &peerStatic) == errSecSuccess, let peerStatic,
          SecCodeCopySigningInformation(peerStatic, informationFlags, &peerInfo) == errSecSuccess,
          SecCodeCopySelf([], &own) == errSecSuccess, let own,
          SecCodeCopyStaticCode(own, [], &ownStatic) == errSecSuccess, let ownStatic,
          SecCodeCopySigningInformation(ownStatic, informationFlags, &ownInfo) == errSecSuccess,
          let peerDictionary = peerInfo as? [String: Any], let ownDictionary = ownInfo as? [String: Any]
    else { return 1 }
    let peerTeam = peerDictionary[kSecCodeInfoTeamIdentifier as String] as? String
    let ownTeam = ownDictionary[kSecCodeInfoTeamIdentifier as String] as? String
    if let ownTeam {
        guard peerTeam == ownTeam else { return 1 }
        // A matching team string alone is not a trust anchor: reject a
        // self-signed identity that merely claims the same organizational unit.
        guard ownTeam.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }) else { return 1 }
        var requirement: SecRequirement?
        let rule = "anchor apple generic and certificate leaf[subject.OU] = \"\(ownTeam)\""
        guard SecRequirementCreateWithString(rule as CFString, [], &requirement) == errSecSuccess,
              let requirement, SecCodeCheckValidity(peer, [], requirement) == errSecSuccess else { return 1 }
    } else {
        #if DEBUG
        // Local ad-hoc bundles retain strict executable-path and code-validity
        // checks. Runtime transport also binds a one-run credential and PID.
        guard peerTeam == nil else { return 1 }
        #else
        return 1
        #endif
    }
    let identifier = peerDictionary[kSecCodeInfoIdentifier as String] as? String
    if role == 1 { return identifier == "ditch_cli" ? 0 : 1 }
    let ownIdentifier = ownDictionary[kSecCodeInfoIdentifier as String] as? String ?? ""
    let uiIdentifier = ownIdentifier.replacingOccurrences(of: ".runtime", with: ".app")
    return identifier == uiIdentifier ? 0 : 1
}
