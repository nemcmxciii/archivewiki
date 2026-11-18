// Tiny helper used by ArchiveWiki's app lock. Asks macOS to confirm the user (Touch ID, Apple Watch, or the
// login password). Exit codes: 0 = confirmed, 1 = cancelled or failed, 2 = this Mac can't authenticate.
import Foundation
import LocalAuthentication

let reason = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "Unlock ArchiveWiki"
let context = LAContext()
var policyError: NSError?

guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &policyError) else {
    exit(2)
}

let done = DispatchSemaphore(value: 0)
var granted = false
context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) { success, _ in
    granted = success
    done.signal()
}
done.wait()
exit(granted ? 0 : 1)
