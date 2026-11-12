import Foundation

final class SyncService {
    func sync(items: [Item]) {
        logger.info("syncing %d items", items.count)
        for item in items {
            upload(item)
        }
    }
}
