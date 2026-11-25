// A small graph library: the property suite's C++ sample.
#include <algorithm>
#include <iostream>
#include <map>
#include <optional>
#include <queue>
#include <stdexcept>
#include <string>
#include <unordered_map>
#include <vector>

namespace graphs {

struct Edge {
    int to;
    double weight;
};

class GraphError : public std::runtime_error {
public:
    explicit GraphError(const std::string& what) : std::runtime_error(what) {}
};

template <typename Label>
class Graph {
public:
    using Id = int;

    Id add(const Label& label) {
        labels_.push_back(label);
        edges_.emplace_back();
        index_[label] = static_cast<Id>(labels_.size() - 1);
        return static_cast<Id>(labels_.size() - 1);
    }

    void connect(Id from, Id to, double weight = 1.0) {
        check(from);
        check(to);
        edges_[from].push_back(Edge{to, weight});
    }

    std::optional<Id> find(const Label& label) const {
        auto it = index_.find(label);
        if (it == index_.end()) {
            return std::nullopt;
        }
        return it->second;
    }

    std::size_t size() const noexcept { return labels_.size(); }

    const Label& label(Id id) const {
        check(id);
        return labels_[id];
    }

    std::vector<double> distances(Id source) const {
        check(source);
        std::vector<double> dist(size(), kInfinity);
        using Item = std::pair<double, Id>;
        std::priority_queue<Item, std::vector<Item>, std::greater<Item>> queue;
        dist[source] = 0.0;
        queue.push({0.0, source});
        while (!queue.empty()) {
            auto [d, at] = queue.top();
            queue.pop();
            if (d > dist[at]) {
                continue;
            }
            for (const Edge& edge : edges_[at]) {
                double next = d + edge.weight;
                if (next < dist[edge.to]) {
                    dist[edge.to] = next;
                    queue.push({next, edge.to});
                }
            }
        }
        return dist;
    }

    std::vector<Id> reachable(Id source) const {
        std::vector<Id> out;
        auto dist = distances(source);
        for (Id id = 0; id < static_cast<Id>(dist.size()); ++id) {
            if (dist[id] < kInfinity) {
                out.push_back(id);
            }
        }
        std::sort(out.begin(), out.end(), [this](Id a, Id b) { return labels_[a] < labels_[b]; });
        return out;
    }

private:
    static constexpr double kInfinity = 1e18;

    void check(Id id) const {
        if (id < 0 || id >= static_cast<Id>(labels_.size())) {
            throw GraphError("no node " + std::to_string(id));
        }
    }

    std::vector<Label> labels_;
    std::vector<std::vector<Edge>> edges_;
    std::map<Label, Id> index_;
};

enum class Mode { Shortest, Reachable };

std::string describe(Mode mode) {
    switch (mode) {
    case Mode::Shortest:
        return "shortest paths";
    case Mode::Reachable:
        return "reachable nodes";
    }
    return "unknown";
}

}  // namespace graphs

int main() {
    graphs::Graph<std::string> g;
    auto home = g.add("home");
    auto shop = g.add("shop");
    auto park = g.add("park");
    g.connect(home, shop, 2.5);
    g.connect(shop, park, 1.0);
    g.connect(home, park, 5.0);
    auto dist = g.distances(home);
    for (std::size_t i = 0; i < dist.size(); ++i) {
        std::cout << g.label(static_cast<int>(i)) << ": " << dist[i] << "\n";
    }
    try {
        g.connect(home, 42);
    } catch (const graphs::GraphError& e) {
        std::cerr << "error: " << e.what() << std::endl;
    }
    std::unordered_map<std::string, int> visits{{"home", 1}};
    visits["park"] += 2;
    std::cout << graphs::describe(graphs::Mode::Reachable) << " " << g.reachable(home).size() << "\n";
    return 0;
}
