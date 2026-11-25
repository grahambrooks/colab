// A weather station: the property suite's Swift sample.
import Foundation

enum Condition: String, CaseIterable {
    case sunny, cloudy, rainy, snowy

    var symbol: String {
        switch self {
        case .sunny: return "☀"
        case .cloudy: return "☁"
        case .rainy: return "☂"
        case .snowy: return "❄"
        }
    }
}

struct Reading: Equatable, Codable {
    let station: String
    let celsius: Double
    let humidity: Int
    let condition: String
    let at: Date
}

enum StationError: Error {
    case unknownStation(String)
    case badReading(reason: String)
}

protocol ReadingSource {
    func readings(for station: String) async throws -> [Reading]
}

final class Station {
    let name: String
    private(set) var readings: [Reading] = []
    private var observers: [(Reading) -> Void] = []

    init(name: String) {
        self.name = name
    }

    var average: Double? {
        guard !readings.isEmpty else { return nil }
        return readings.map(\.celsius).reduce(0, +) / Double(readings.count)
    }

    func observe(_ observer: @escaping (Reading) -> Void) {
        observers.append(observer)
    }

    func record(_ reading: Reading) throws {
        guard reading.station == name else {
            throw StationError.unknownStation(reading.station)
        }
        if reading.humidity < 0 || reading.humidity > 100 {
            throw StationError.badReading(reason: "humidity \(reading.humidity)")
        }
        readings.append(reading)
        observers.forEach { $0(reading) }
    }

    func hottest(_ count: Int = 3) -> [Reading] {
        Array(readings.sorted { $0.celsius > $1.celsius }.prefix(count))
    }

    func summary() -> String {
        let counts = Dictionary(grouping: readings, by: { $0.condition }).mapValues(\.count)
        return counts
            .sorted { $0.key < $1.key }
            .map { "\($0.key): \($0.value)" }
            .joined(separator: ", ")
    }

    static func describe(_ celsius: Double) -> String {
        switch celsius {
        case ..<0:
            return "freezing"
        case 0..<15:
            return "cold"
        case 15..<25:
            return "mild"
        default:
            return "hot"
        }
    }
}

extension Station: CustomStringConvertible {
    var description: String {
        "\(name): \(readings.count) readings, average \(average.map { String(format: "%.1f", $0) } ?? "n/a")"
    }
}

struct Forecast<Source: ReadingSource> {
    let source: Source
    let stations: [String]

    func load() async -> [String: [Reading]] {
        var out: [String: [Reading]] = [:]
        for station in stations {
            do {
                out[station] = try await source.readings(for: station)
            } catch {
                print("skipped \(station): \(error)")
            }
        }
        return out
    }
}

@discardableResult
func makeReading(_ station: String, _ celsius: Double, humidity: Int = 50) -> Reading {
    Reading(station: station, celsius: celsius, humidity: humidity, condition: "sunny", at: Date())
}

let station = Station(name: "harbour")
station.observe { reading in
    print("\(reading.station): \(Station.describe(reading.celsius))")
}
do {
    try station.record(makeReading("harbour", 18.5))
    try station.record(makeReading("harbour", -2.0, humidity: 80))
    try station.record(makeReading("airport", 12.0))
} catch StationError.unknownStation(let name) {
    print("no station \(name)")
} catch {
    print("failed: \(error)")
}
print(station, station.hottest(1).first?.celsius ?? 0)
