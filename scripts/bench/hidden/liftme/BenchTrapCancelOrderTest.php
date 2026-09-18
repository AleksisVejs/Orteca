<?php

namespace Tests\Feature;

use App\Models\Category;
use App\Models\Order;
use App\Models\Product;
use App\Models\ProductVariant;
use App\Models\User;
use App\Services\OrderService;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Tests\TestCase;

class BenchTrapCancelOrderTest extends TestCase
{
    use RefreshDatabase;

    private function user(string $role = User::ROLE_CUSTOMER): User
    {
        return User::create([
            'name' => 'Buyer', 'email' => 'u-'.uniqid().'@acme.test', 'password' => 'password',
            'role' => $role, 'is_active' => true,
        ]);
    }

    private function product(int $stock = 10): Product
    {
        $category = Category::create(['name' => ['en' => 'Hoists'], 'slug' => 'hoists-'.uniqid()]);

        return Product::create([
            'category_id' => $category->id, 'sku' => 'P-'.uniqid(), 'name' => ['en' => 'Hoist'],
            'slug' => 'p-'.uniqid(), 'base_price' => 100, 'currency' => 'EUR', 'is_active' => true,
            'allow_direct_purchase' => true, 'manage_stock' => true, 'stock_quantity' => $stock,
        ]);
    }

    private function order(?User $user, Product $product, float $qty, array $attrs = [], ?ProductVariant $variant = null): Order
    {
        $order = Order::create(array_merge([
            'order_number' => 'O-'.uniqid(), 'user_id' => $user?->id, 'type' => Order::TYPE_DIRECT,
            'status' => Order::STATUS_PENDING, 'payment_status' => 'unpaid', 'currency' => 'EUR',
        ], $attrs));
        $order->items()->create([
            'product_id' => $product->id, 'variant_id' => $variant?->id, 'name' => 'Hoist', 'sku' => $product->sku,
            'quantity' => $qty, 'unit_price' => 100, 'line_total' => 100 * $qty,
        ]);

        return $order->fresh('items');
    }

    public function test_owner_cancels_an_unpaid_order(): void
    {
        $owner = $this->user();
        foreach ([Order::STATUS_PENDING, Order::STATUS_AWAITING_PAYMENT] as $status) {
            $order = $this->order($owner, $this->product(), 1, ['status' => $status]);
            $this->actingAs($owner)->postJson("/api/orders/{$order->id}/cancel")
                ->assertOk()->assertJsonPath('data.status', Order::STATUS_CANCELLED);
            $this->assertSame(Order::STATUS_CANCELLED, $order->fresh()->status);
        }
    }

    public function test_only_the_owning_customer_may_cancel(): void
    {
        $owner = $this->user();
        $order = $this->order($owner, $this->product(), 1);
        $guestOrder = $this->order(null, $this->product(), 1);

        $this->postJson("/api/orders/{$order->id}/cancel")->assertUnauthorized();
        $this->actingAs($this->user())->postJson("/api/orders/{$order->id}/cancel")->assertForbidden();
        $this->actingAs($this->user(User::ROLE_ADMIN))->postJson("/api/orders/{$order->id}/cancel")->assertForbidden();
        $this->actingAs($owner)->postJson("/api/orders/{$guestOrder->id}/cancel")->assertForbidden();

        $this->assertSame(Order::STATUS_PENDING, $order->fresh()->status);
        $this->assertSame(Order::STATUS_PENDING, $guestOrder->fresh()->status);
    }

    public function test_paid_or_progressed_orders_cannot_be_cancelled(): void
    {
        $owner = $this->user();
        $cases = [
            ['status' => Order::STATUS_PAID, 'payment_status' => 'paid'],
            ['status' => Order::STATUS_PROCESSING],
            ['status' => Order::STATUS_SHIPPED],
            ['status' => Order::STATUS_COMPLETED],
            ['status' => Order::STATUS_CANCELLED],
            // The Stripe webhook can mark a still-pending order paid.
            ['status' => Order::STATUS_PENDING, 'payment_status' => 'paid'],
        ];
        foreach ($cases as $attrs) {
            $order = $this->order($owner, $this->product(), 1, $attrs);
            $this->actingAs($owner)->postJson("/api/orders/{$order->id}/cancel")->assertStatus(422);
            $this->assertSame($attrs['status'], $order->fresh()->status);
        }
    }

    public function test_cancelling_an_unpaid_order_moves_no_stock(): void
    {
        $owner = $this->user();
        $product = $this->product(10);
        $order = $this->order($owner, $product, 3);

        $this->actingAs($owner)->postJson("/api/orders/{$order->id}/cancel")->assertOk();

        $this->assertSame(10, $product->fresh()->stock_quantity);
    }

    public function test_committed_stock_comes_back_exactly_once(): void
    {
        $product = $this->product(10);
        $order = $this->order($this->user(), $product, 3);
        $service = app(OrderService::class);

        $service->updateStatus($order, Order::STATUS_PAID);
        $this->assertSame(7, $product->fresh()->stock_quantity);

        $service->updateStatus($order->fresh('items'), Order::STATUS_CANCELLED);
        $this->assertSame(10, $product->fresh()->stock_quantity);

        $service->updateStatus($order->fresh('items'), Order::STATUS_CANCELLED);
        $service->updateStatus($order->fresh('items'), Order::STATUS_COMPLETED);
        $service->updateStatus($order->fresh('items'), Order::STATUS_PAID);
        $this->assertSame(10, $product->fresh()->stock_quantity);
    }

    public function test_variant_stock_goes_back_to_the_variant(): void
    {
        $product = $this->product(10);
        $variant = ProductVariant::create([
            'product_id' => $product->id, 'sku' => 'V-'.uniqid(), 'price' => 120,
            'manage_stock' => true, 'stock_quantity' => 5, 'is_active' => true,
        ]);
        $order = $this->order($this->user(), $product, 2, [], $variant);
        $service = app(OrderService::class);

        $service->updateStatus($order, Order::STATUS_PAID);
        $this->assertSame(3, $variant->fresh()->stock_quantity);

        $service->updateStatus($order->fresh('items'), Order::STATUS_CANCELLED);
        $this->assertSame(5, $variant->fresh()->stock_quantity);
        $this->assertSame(10, $product->fresh()->stock_quantity);
    }
}
