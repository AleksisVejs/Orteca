<?php

namespace Tests\Feature;

use App\Models\Conversation;
use App\Models\Message;
use App\Models\User;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Facades\DB;
use Illuminate\Support\Facades\Event;
use Tests\TestCase;

class BenchMediumConversationListTest extends TestCase
{
    use RefreshDatabase;

    private function thread(User $user, int $messages, int $unreadAdmin = 0): Conversation
    {
        $c = Conversation::create(['user_id' => $user->id, 'subject' => 'T', 'status' => 'open', 'last_message_at' => now()]);
        for ($i = 1; $i <= $unreadAdmin; $i++) {
            $m = new Message(['conversation_id' => $c->id, 'sender_type' => 'admin', 'body' => "c{$c->id} old admin $i"]);
            $m->created_at = now()->subMinutes(2000 + $i);
            $m->save();
        }

        for ($i = 1; $i <= $messages; $i++) {
            $m = new Message(['conversation_id' => $c->id, 'sender_type' => 'customer', 'body' => "c{$c->id} m$i"]);
            $m->created_at = now()->subMinutes(1000 - $i);
            $m->save();
        }

        return $c;
    }

    private int $queries = 0;

    private int $hydrated = 0;

    /** @return array{int, int} queries, hydrated messages */
    private function measure(User $user): array
    {
        $this->queries = $this->hydrated = 0;
        $this->actingAs($user)->getJson('/api/conversations')->assertOk();

        return [$this->queries, $this->hydrated];
    }

    public function test_list_shows_latest_message_and_unread_without_full_history(): void
    {
        $user = User::factory()->create();
        $a = $this->thread($user, 25, 2);
        $b = $this->thread($user, 3);

        $rows = collect($this->actingAs($user)->getJson('/api/conversations')->assertOk()->json('data'))->keyBy('id');
        $this->assertSame("c{$a->id} m25", $rows[$a->id]['latest_message']);
        $this->assertSame("c{$b->id} m3", $rows[$b->id]['latest_message']);
        $this->assertSame(2, $rows[$a->id]['unread_count']);
        $this->assertSame(0, $rows[$b->id]['unread_count']);
        $this->assertArrayNotHasKey('messages', $rows[$a->id]);

        $this->actingAs($user)->getJson("/api/conversations/{$a->id}")->assertOk()->assertJsonCount(27, 'data.messages');
    }

    public function test_cost_does_not_grow_with_threads_or_history(): void
    {
        DB::listen(function () { $this->queries++; });
        Event::listen('eloquent.retrieved: '.Message::class, function () { $this->hydrated++; });
        $small = User::factory()->create();
        $this->thread($small, 2);
        [$q1] = $this->measure($small);

        $big = User::factory()->create();
        for ($i = 0; $i < 6; $i++) {
            $this->thread($big, 30);
        }
        [$q2, $hydrated] = $this->measure($big);

        $this->assertSame($q1, $q2, "queries grew from $q1 to $q2");
        $this->assertLessThanOrEqual(6, $hydrated, "hydrated $hydrated messages for 6 threads");
    }
}
