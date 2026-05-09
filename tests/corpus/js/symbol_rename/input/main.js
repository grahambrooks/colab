function oldHelper(x) {
    return x + 1;
}

const result = oldHelper(oldHelper(1));
console.log(result);
