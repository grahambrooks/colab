import Foundation

final class SyncService {
    func sync(items: [Item]) {
        NSLog("syncing %d items", items.count)
        print("debug", items)
        for item in items {
            upload(item)
        }
    }
}
