#include <map>
#include <string>
#include <utility>

void Index::add(const std::string& key, int value) {
    entries_.insert(std::make_pair(key, value));
}

std::pair<std::string, int> Index::first() const {
    auto it = entries_.begin();
    return std::make_pair(it->first, it->second);
}
