<?php
// A blog engine: the property suite's PHP sample.

declare(strict_types=1);

namespace Example\Blog;

use DateTimeImmutable;
use InvalidArgumentException;
use JsonSerializable;

enum Status: string
{
    case Draft = 'draft';
    case Published = 'published';
    case Archived = 'archived';
}

interface Repository
{
    public function find(int $id): ?Post;

    public function save(Post $post): void;
}

final class Post implements JsonSerializable
{
    private array $tags = [];

    public function __construct(
        public readonly int $id,
        private string $title,
        private string $body,
        private Status $status = Status::Draft,
        private ?DateTimeImmutable $published = null,
    ) {
        if (trim($title) === '') {
            throw new InvalidArgumentException('a post needs a title');
        }
    }

    public function title(): string
    {
        return $this->title;
    }

    public function publish(DateTimeImmutable $when): self
    {
        $copy = clone $this;
        $copy->status = Status::Published;
        $copy->published = $when;
        return $copy;
    }

    public function tag(string ...$tags): void
    {
        foreach ($tags as $tag) {
            $tag = strtolower(trim($tag));
            if ($tag !== '' && !in_array($tag, $this->tags, true)) {
                $this->tags[] = $tag;
            }
        }
    }

    public function excerpt(int $words = 20): string
    {
        $parts = preg_split('/\s+/', $this->body) ?: [];
        $short = array_slice($parts, 0, $words);
        return implode(' ', $short) . (count($parts) > $words ? '…' : '');
    }

    public function jsonSerialize(): array
    {
        return [
            'id' => $this->id,
            'title' => $this->title,
            'status' => $this->status->value,
            'tags' => $this->tags,
            'published' => $this->published?->format(DATE_ATOM),
        ];
    }

    public function label(): string
    {
        return match ($this->status) {
            Status::Draft => 'not yet',
            Status::Published => 'live',
            Status::Archived => 'old',
        };
    }
}

class MemoryRepository implements Repository
{
    /** @var array<int, Post> */
    private array $posts = [];

    public function find(int $id): ?Post
    {
        return $this->posts[$id] ?? null;
    }

    public function save(Post $post): void
    {
        $this->posts[$post->id] = $post;
    }

    public function all(callable $filter = null): array
    {
        $posts = array_values($this->posts);
        if ($filter !== null) {
            $posts = array_filter($posts, $filter);
        }
        usort($posts, fn (Post $a, Post $b) => strcmp($a->title(), $b->title()));
        return $posts;
    }
}

abstract class Renderer
{
    abstract protected function template(): string;

    public static function escape(string $text): string
    {
        return htmlspecialchars($text, ENT_QUOTES, 'UTF-8');
    }

    public function render(Post $post): string
    {
        $html = str_replace('{title}', self::escape($post->title()), $this->template());
        return str_replace('{excerpt}', self::escape($post->excerpt()), $html);
    }
}

final class CardRenderer extends Renderer
{
    protected function template(): string
    {
        return '<article><h2>{title}</h2><p>{excerpt}</p></article>';
    }
}

function demo(): void
{
    $repo = new MemoryRepository();
    $post = new Post(1, 'Hello', 'The first post on this blog, with a few words.');
    $post->tag('Intro', 'news');
    $repo->save($post->publish(new DateTimeImmutable('2026-11-24')));
    $renderer = new CardRenderer();
    foreach ($repo->all() as $item) {
        echo $renderer->render($item), PHP_EOL;
    }
    switch (count($repo->all())) {
        case 0:
            echo "empty\n";
            break;
        default:
            echo json_encode($repo->find(1)), "\n";
    }
}

demo();
