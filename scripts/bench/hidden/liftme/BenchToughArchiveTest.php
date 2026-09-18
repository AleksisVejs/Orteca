<?php

namespace Tests\Feature;

use App\Models\Conversation;
use App\Models\Message;
use App\Models\User;
use App\Services\ChatService;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Facades\Mail;
use Illuminate\Support\Facades\Schema;
use Tests\TestCase;

class BenchToughArchiveTest extends TestCase
{
    use RefreshDatabase;

    private function thread(User $user, string $subject): Conversation
    {
        return Conversation::create(['user_id' => $user->id, 'subject' => $subject, 'status' => 'open', 'last_message_at' => now()]);
    }

    public function test_column_exists(): void
    {
        $this->assertTrue(Schema::hasColumn('conversations', 'archived_at'));
    }

    public function test_owner_archives_and_list_filters(): void
    {
        $user = User::factory()->create();
        $a = $this->thread($user, 'keep');
        $b = $this->thread($user, 'hide');

        $this->actingAs($user)->postJson("/api/conversations/{$b->id}/archive")
            ->assertSuccessful()->assertJsonPath('data.archived', true);
        $this->assertNotNull($b->fresh()->archived_at);

        $this->actingAs($user)->getJson('/api/conversations')->assertOk()
            ->assertJsonCount(1, 'data')->assertJsonPath('data.0.id', $a->id)
            ->assertJsonPath('data.0.archived', false);
        $this->actingAs($user)->getJson('/api/conversations?archived=1')->assertOk()
            ->assertJsonCount(1, 'data')->assertJsonPath('data.0.id', $b->id);

        $this->actingAs($user)->postJson("/api/conversations/{$b->id}/unarchive")
            ->assertSuccessful()->assertJsonPath('data.archived', false);
        $this->assertNull($b->fresh()->archived_at);
        $this->actingAs($user)->getJson('/api/conversations')->assertOk()->assertJsonCount(2, 'data');
        $this->actingAs($user)->getJson('/api/conversations?archived=1')->assertOk()->assertJsonCount(0, 'data');
    }

    public function test_only_the_owner_may_archive(): void
    {
        $user = User::factory()->create();
        $stranger = User::factory()->create();
        $admin = User::factory()->create(['role' => User::ROLE_ADMIN]);
        $c = $this->thread($user, 'mine');

        foreach ([$stranger, $admin] as $who) {
            $this->actingAs($who)->postJson("/api/conversations/{$c->id}/archive")->assertForbidden();
            $this->actingAs($who)->postJson("/api/conversations/{$c->id}/unarchive")->assertForbidden();
        }
        $this->assertNull($c->fresh()->archived_at);
    }

    public function test_new_messages_unarchive(): void
    {
        Mail::fake();
        $user = User::factory()->create();
        $admin = User::factory()->create(['role' => User::ROLE_ADMIN]);
        $c = $this->thread($user, 'x');

        $this->actingAs($user)->postJson("/api/conversations/{$c->id}/archive")->assertSuccessful();
        $this->actingAs($user)->postJson("/api/conversations/{$c->id}/messages", ['body' => 'back again'])->assertSuccessful();
        $this->assertNull($c->fresh()->archived_at);

        $this->actingAs($user)->postJson("/api/conversations/{$c->id}/archive")->assertSuccessful();
        app(ChatService::class)->postMessage($c->fresh(), 'staff reply', $admin, Message::SENDER_ADMIN);
        $this->assertNull($c->fresh()->archived_at);
    }

    public function test_summary_ignores_archived_threads(): void
    {
        $user = User::factory()->create();
        $open = $this->thread($user, 'open');
        $old = $this->thread($user, 'old');
        $open->messages()->create(['sender_type' => Message::SENDER_ADMIN, 'body' => 'a']);
        $old->messages()->create(['sender_type' => Message::SENDER_ADMIN, 'body' => 'b']);
        $old->messages()->create(['sender_type' => Message::SENDER_ADMIN, 'body' => 'c']);

        $this->actingAs($user)->getJson('/api/account/summary')->assertOk()->assertJsonPath('messages', 3);
        $this->actingAs($user)->postJson("/api/conversations/{$old->id}/archive")->assertSuccessful();
        $this->actingAs($user)->getJson('/api/account/summary')->assertOk()->assertJsonPath('messages', 1);
    }
}
