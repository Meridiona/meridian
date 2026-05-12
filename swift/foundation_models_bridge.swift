// meridian — normalises screenpipe activity into structured app sessions

import Foundation
@preconcurrency import FoundationModels

private func makeCString(_ str: String) -> UnsafeMutablePointer<CChar> {
    return strdup(str)!
}

@available(macOS 26, *)
private enum FM {
    static func checkAvailability(_ out_reason: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> Int32 {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            out_reason.pointee = makeCString("available")
            return 0
        case .unavailable(let reason):
            switch reason {
            case .appleIntelligenceNotEnabled:
                out_reason.pointee = makeCString("Apple Intelligence is not enabled")
                return 1
            case .deviceNotEligible:
                out_reason.pointee = makeCString("Device not eligible")
                return 2
            case .modelNotReady:
                out_reason.pointee = makeCString("Model not ready")
                return 3
            @unknown default:
                out_reason.pointee = makeCString("Unknown")
                return 4
            }
        }
    }

    static func generateText(
        _ instructions: UnsafePointer<CChar>?,
        _ prompt: UnsafePointer<CChar>?,
        _ out_text: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
        _ out_error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
    ) -> Int32 {
        guard let prompt = prompt else {
            out_error.pointee = makeCString("prompt is null")
            return -1
        }
        let promptStr = String(cString: prompt)
        let instructionsStr = instructions.map { String(cString: $0) }
        let semaphore = DispatchSemaphore(value: 0)
        var status: Int32 = 0
        Task {
            do {
                let model = SystemLanguageModel(guardrails: .permissiveContentTransformations)
                let session: LanguageModelSession
                if let inst = instructionsStr {
                    session = LanguageModelSession(model: model, instructions: inst)
                } else {
                    session = LanguageModelSession(model: model)
                }
                let response = try await session.respond(to: promptStr)
                out_text.pointee = makeCString(response.content)
                status = 0
            } catch {
                out_error.pointee = makeCString(error.localizedDescription)
                status = -1
            }
            semaphore.signal()
        }
        semaphore.wait()
        return status
    }

    static func prewarm() -> Int32 {
        let model = SystemLanguageModel.default
        guard model.availability == .available else { return -1 }
        LanguageModelSession().prewarm()
        return 0
    }
}

@_cdecl("fm_check_availability")
public func fmCheckAvailability(_ out_reason: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> Int32 {
    if #available(macOS 26, *) { return FM.checkAvailability(out_reason) }
    out_reason.pointee = makeCString("macOS 26 or later required")
    return 4
}

@_cdecl("fm_free_string")
public func fmFreeString(_ ptr: UnsafeMutablePointer<CChar>?) {
    if let ptr = ptr { free(ptr) }
}

@_cdecl("fm_generate_text")
public func fmGenerateText(
    _ instructions: UnsafePointer<CChar>?,
    _ prompt: UnsafePointer<CChar>?,
    _ out_text: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
    _ out_error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    if #available(macOS 26, *) { return FM.generateText(instructions, prompt, out_text, out_error) }
    out_error.pointee = makeCString("macOS 26 or later required")
    return -1
}

@_cdecl("fm_prewarm")
public func fmPrewarm() -> Int32 {
    if #available(macOS 26, *) { return FM.prewarm() }
    return -1
}
