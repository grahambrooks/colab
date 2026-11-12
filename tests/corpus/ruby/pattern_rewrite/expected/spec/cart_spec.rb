require "cart"

RSpec.describe Cart do
  it "totals its items" do
    cart = Cart.new
    cart.add(:apple, 2)
    expect(cart.count).to eq(2)
    expect(cart.total).to eq(Cart.price(:apple) * 2)
  end
end
