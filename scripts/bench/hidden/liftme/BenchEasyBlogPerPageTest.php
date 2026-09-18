<?php

namespace Tests\Feature;

use App\Models\BlogPost;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Tests\TestCase;

class BenchEasyBlogPerPageTest extends TestCase
{
    use RefreshDatabase;

    protected function setUp(): void
    {
        parent::setUp();
        for ($i = 1; $i <= 40; $i++) {
            BlogPost::create([
                'title' => ['en' => "Post $i"], 'slug' => "post-$i",
                'status' => BlogPost::STATUS_PUBLISHED, 'published_at' => now()->subDays($i),
            ]);
        }
    }

    public function test_missing_zero_and_negative_use_the_default(): void
    {
        foreach (['', '?per_page=0', '?per_page=-1', '?per_page=-50'] as $q) {
            $this->getJson('/api/blog/posts'.$q)->assertOk()
                ->assertJsonCount(12, 'data')
                ->assertJsonPath('meta.per_page', 12);
        }
    }

    public function test_large_values_are_capped_and_valid_ones_kept(): void
    {
        $this->getJson('/api/blog/posts?per_page=100')->assertOk()->assertJsonCount(30, 'data');
        $this->getJson('/api/blog/posts?per_page=5')->assertOk()->assertJsonCount(5, 'data')
            ->assertJsonPath('meta.per_page', 5);
        $this->getJson('/api/blog/posts?per_page=12&page=4')->assertOk()->assertJsonCount(4, 'data');
    }
}
