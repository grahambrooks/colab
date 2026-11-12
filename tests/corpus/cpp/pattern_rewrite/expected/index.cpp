#include <map>
#include <string>
#include <utility>

void Index::add(const std::string& key, int value) {
    entries_.insert(std::pair(key, value));
}

std::pair<std::string, int> Index::first() const {
    auto it = entries_.begin();
    return std::pair(it->first, it->second);
}
