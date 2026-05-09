function newHelper(x) {
    return x + 1;
}

const result = newHelper(newHelper(1));
console.log(result);
