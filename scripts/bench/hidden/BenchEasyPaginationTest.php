<?php

namespace Tests\Feature;

use App\Models\Admin;
use App\Models\Client;
use App\Models\Equipment;
use App\Models\User;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Facades\DB;
use Illuminate\Support\Facades\Hash;
use Tests\TestCase;

class BenchEasyPaginationTest extends TestCase
{
    use RefreshDatabase;

    private Admin $admin;

    private User $adminUser;

    protected function setUp(): void
    {
        parent::setUp();
        $this->adminUser = $this->makeUser('admin');
        $this->admin = Admin::create([
            'user_id' => $this->adminUser->id,
            'company_name' => 'Page Co',
            'company_registration_number' => '40000000002',
            'verification_status' => 'verified',
        ]);
        $this->entitle($this->admin);
        foreach (['A', 'B', 'C'] as $name) {
            Equipment::create(['name' => "Crane $name", 'admin_id' => $this->admin->id]);
        }
    }

    private function makeUser(string $type): User
    {
        return User::factory()->create([
            'type' => $type,
            'password' => Hash::make('Password1!'),
            'email_verified_at' => now(),
            'is_active' => true,
        ]);
    }

    public function test_missing_per_page_is_paginated_at_50(): void
    {
        $response = $this->actingAs($this->adminUser)->getJson('/api/equipment')->assertOk();
        $this->assertEquals(50, $response->json('per_page'));
        $this->assertCount(3, $response->json('data'));
        $this->assertEquals(3, $response->json('total'));
    }

    public function test_zero_per_page_is_paginated_at_50(): void
    {
        $response = $this->actingAs($this->adminUser)->getJson('/api/equipment?per_page=0')->assertOk();
        $this->assertEquals(50, $response->json('per_page'));
        $this->assertCount(3, $response->json('data'));
    }

    public function test_per_page_is_capped_at_100(): void
    {
        $response = $this->actingAs($this->adminUser)->getJson('/api/equipment?per_page=500')->assertOk();
        $this->assertEquals(100, $response->json('per_page'));
    }

    public function test_explicit_per_page_is_respected(): void
    {
        $response = $this->actingAs($this->adminUser)->getJson('/api/equipment?per_page=2')->assertOk();
        $this->assertEquals(2, $response->json('per_page'));
        $this->assertCount(2, $response->json('data'));
        $this->assertEquals(3, $response->json('total'));
    }

    public function test_client_list_is_paginated_too(): void
    {
        $clientUser = $this->makeUser('client');
        Client::create(['user_id' => $clientUser->id, 'admin_id' => $this->admin->id, 'company_name' => 'Holder']);
        DB::table('client_equipment')->insert([
            'equipment_id' => Equipment::where('name', 'Crane A')->value('id'),
            'client_id' => $clientUser->id,
        ]);

        $response = $this->actingAs($clientUser)->getJson('/api/equipment')->assertOk();
        $this->assertEquals(50, $response->json('per_page'));
        $this->assertCount(1, $response->json('data'));
    }
}
