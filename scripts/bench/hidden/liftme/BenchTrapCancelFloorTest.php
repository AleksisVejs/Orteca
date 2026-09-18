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

class BenchTrapCancelFloorTest extends TestCase
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

    /** Found by Orteca's Review on 2026-09-18: taking stock floors at 0, so give back only what was taken. */
    public function test_restock_returns_only_what_was_taken(): void
    {
        $product = $this->product(1);
        $order = $this->order($this->user(), $product, 3);
        $service = app(OrderService::class);

        $service->updateStatus($order, Order::STATUS_PAID);
        $this->assertSame(0, $product->fresh()->stock_quantity);

        $service->updateStatus($order->fresh('items'), Order::STATUS_CANCELLED);
        $this->assertSame(1, $product->fresh()->stock_quantity);
    }
}
