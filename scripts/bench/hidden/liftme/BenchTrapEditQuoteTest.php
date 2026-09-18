<?php

namespace Tests\Feature;

use App\Models\Quote;
use App\Models\QuoteItem;
use App\Models\User;
use App\Services\QuoteService;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Tests\TestCase;

class BenchTrapEditQuoteTest extends TestCase
{
    use RefreshDatabase;

    private function user(string $role = User::ROLE_CUSTOMER): User
    {
        return User::create([
            'name' => 'Buyer', 'email' => 'u-'.uniqid().'@acme.test', 'password' => 'password',
            'role' => $role, 'is_active' => true,
        ]);
    }

    private function quote(?User $user, int $lines = 2): Quote
    {
        $items = [];
        for ($i = 1; $i <= $lines; $i++) {
            $items[] = ['name' => "Line $i", 'sku' => "S$i", 'quantity' => $i];
        }

        return app(QuoteService::class)->createRequest(
            items: $items,
            contact: ['contact_name' => 'Jane', 'contact_email' => 'jane@example.com'],
            user: $user,
        );
    }

    public function test_owner_edits_a_pending_quote(): void
    {
        $owner = $this->user();
        $quote = $this->quote($owner);
        [$keep, $drop] = $quote->items->all();

        $this->actingAs($owner)->patchJson("/api/quotes/{$quote->id}", [
            'customer_note' => 'Need it by Friday',
            'delivery_method' => Quote::DELIVERY_PICKUP,
            'items' => [['id' => $keep->id, 'quantity' => 5]],
        ])->assertOk()->assertJsonCount(1, 'data.items');

        $quote->refresh();
        $this->assertSame('Need it by Friday', $quote->customer_note);
        $this->assertSame(Quote::DELIVERY_PICKUP, $quote->delivery_method);
        $this->assertEquals(5, (float) $keep->fresh()->quantity);
        $this->assertNull(QuoteItem::find($drop->id));
        $this->assertTrue($quote->events()->where('type', 'edited')->exists());
    }

    public function test_prices_status_and_other_fields_cannot_be_set(): void
    {
        $owner = $this->user();
        $other = $this->user();
        $quote = $this->quote($owner);
        $token = $quote->access_token;
        $line = $quote->items->first();

        $this->actingAs($owner)->patchJson("/api/quotes/{$quote->id}", [
            'status' => Quote::STATUS_ACCEPTED, 'total' => 1, 'subtotal' => 1, 'admin_note' => 'approved',
            'user_id' => $other->id, 'access_token' => 'x', 'quote_number' => 'Q-FAKE', 'valid_until' => '2099-01-01',
            'items' => [
                ['id' => $line->id, 'quantity' => 2, 'unit_price' => 1, 'line_total' => 1, 'name' => 'Free', 'product_id' => 999, 'quote_id' => 999],
                ['id' => $quote->items->last()->id, 'quantity' => 1],
            ],
        ])->assertOk();

        $quote->refresh();
        $this->assertSame(Quote::STATUS_PENDING, $quote->status);
        $this->assertNull($quote->admin_note);
        $this->assertEquals(0, (float) $quote->total);
        $this->assertSame($owner->id, $quote->user_id);
        $this->assertSame($token, $quote->access_token);
        $this->assertNotSame('Q-FAKE', $quote->quote_number);
        $this->assertNull($quote->valid_until);

        $line->refresh();
        $this->assertEquals(2, (float) $line->quantity);
        $this->assertNull($line->unit_price);
        $this->assertSame('Line 1', $line->name);
        $this->assertNull($line->product_id);
        $this->assertSame($quote->id, (int) $line->quote_id);
    }

    public function test_lines_of_another_quote_cannot_be_touched(): void
    {
        $owner = $this->user();
        $quote = $this->quote($owner);
        $foreign = $this->quote($this->user(), 1)->items->first();

        $this->actingAs($owner)->patchJson("/api/quotes/{$quote->id}", [
            'items' => [['id' => $foreign->id, 'quantity' => 9]],
        ])->assertStatus(422);

        $foreign->refresh();
        $this->assertEquals(1, (float) $foreign->quantity);
        $this->assertNotSame($quote->id, (int) $foreign->quote_id);
        $this->assertCount(2, $quote->fresh()->items);
    }

    public function test_only_the_owner_may_edit(): void
    {
        $owner = $this->user();
        $quote = $this->quote($owner);
        $guestQuote = $this->quote(null);
        $body = ['customer_note' => 'changed'];

        $this->patchJson("/api/quotes/{$quote->id}", $body)->assertUnauthorized();
        $this->actingAs($this->user())->patchJson("/api/quotes/{$quote->id}", $body)->assertForbidden();
        $this->actingAs($this->user(User::ROLE_ADMIN))->patchJson("/api/quotes/{$quote->id}", $body)->assertForbidden();
        $this->actingAs($owner)->patchJson("/api/quotes/{$guestQuote->id}", $body)->assertForbidden();

        $this->assertNull($quote->fresh()->customer_note);
        $this->assertNull($guestQuote->fresh()->customer_note);
    }

    public function test_only_pending_quotes_can_be_edited(): void
    {
        $owner = $this->user();
        foreach ([Quote::STATUS_SENT, Quote::STATUS_ACCEPTED, Quote::STATUS_REJECTED, Quote::STATUS_EXPIRED, Quote::STATUS_CONVERTED] as $status) {
            $quote = $this->quote($owner);
            $quote->forceFill(['status' => $status])->save();

            $this->actingAs($owner)->patchJson("/api/quotes/{$quote->id}", [
                'customer_note' => 'late',
                'items' => [['id' => $quote->items->first()->id, 'quantity' => 7]],
            ])->assertStatus(422);

            $this->assertNull($quote->fresh()->customer_note);
            $this->assertEquals(1, (float) $quote->items->first()->fresh()->quantity);
            $this->assertCount(2, $quote->fresh()->items);
        }
    }

    public function test_bad_input_is_rejected(): void
    {
        $owner = $this->user();
        $quote = $this->quote($owner);
        $line = $quote->items->first();

        foreach ([
            ['items' => []],
            ['items' => [['id' => $line->id, 'quantity' => 0]]],
            ['items' => [['id' => $line->id, 'quantity' => -2]]],
            ['delivery_method' => 'drone'],
        ] as $body) {
            $this->actingAs($owner)->patchJson("/api/quotes/{$quote->id}", $body)->assertStatus(422);
        }

        $this->assertCount(2, $quote->fresh()->items);
        $this->assertSame(Quote::DELIVERY_SHIPPING, $quote->fresh()->delivery_method);
    }
}
