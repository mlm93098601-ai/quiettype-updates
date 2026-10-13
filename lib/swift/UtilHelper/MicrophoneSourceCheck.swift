import Foundation
import CoreAudio

@main struct MicrophoneSourceCheck {
    static func main() {
        let builtIn = microphoneKind(transport: kAudioDeviceTransportTypeBuiltIn,
                                    inputTerminals: [kAudioStreamTerminalTypeMicrophone])
        assert(builtIn.builtIn && !builtIn.headset)
        let bluetooth = microphoneKind(transport: kAudioDeviceTransportTypeBluetooth, inputTerminals: [])
        assert(bluetooth.headset && !bluetooth.builtIn)
        let webcam = microphoneKind(transport: kAudioDeviceTransportTypeUSB,
                                   inputTerminals: [kAudioStreamTerminalTypeMicrophone])
        assert(!webcam.headset && !webcam.builtIn)
        let headset = microphoneKind(transport: kAudioDeviceTransportTypeUSB,
                                    inputTerminals: [kAudioStreamTerminalTypeHeadsetMicrophone])
        assert(headset.headset)
        let required = microphoneDevicesJSON(nil, 0)
        var bytes = [CChar](repeating: 0, count: Int(required))
        assert(microphoneDevicesJSON(&bytes, required) <= required)
        let data = Data(bytes: bytes, count: max(0, bytes.count - 1))
        guard let devices = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            fatalError("Invalid microphone metadata")
        }
        print("Read-only input metadata: builtIn=\(devices.filter { $0["builtIn"] as? Bool == true }.count), headset=\(devices.filter { $0["headset"] as? Bool == true }.count), total=\(devices.count)")
        print("Microphone transport/terminal classification and read-only metadata passed")
    }
}
